# worklog

A running log of autonomous product work: what changed, why, and what the
evidence was. Newest entries first. Every entry names its proof — if a claim
here has no test or drill behind it, treat it as wrong.

## 2026-08-26 — An issue title no longer shares another product's erasure key

The shipped PostgreSQL Issues path encrypted each title, but selected the old unscoped subject key.
Destroying that key for an Issues request would also make legacy Chat or agent data for the same
person unreadable; preserving it would leave the issue recoverable. That made a truthful Issues
privacy holder impossible even though the ciphertext itself was sound.

Subject keys now admit an explicit Issues class. Every new Issue free-text envelope uses
`scoped-subject:issues:<person>`, while decryption continues to understand legacy references so an
upgrade does not discard data. The Issues module owns the exact current and legacy class predicate
that its future eraser will use. Tests prove the same human's Issues, Chat, and agent-data keys are
distinct, and the live PostgreSQL saga inspects the authoritative row rather than inferring its key
from an in-memory encryption result.

This is deliberately a foundation, not an erasure claim. Existing unscoped issue rows still need an
explicit re-key or fail-loud migration path; authored-title tombstoning, holder receipts, post-PIT
ledger replay, and the public request scope remain open until they can be proved together.

**Proof:** the focused Storage key grammar and Issues encryption suites; the external Issues
pseudonym/DEK contract; strict all-target/all-feature Storage and Issues Clippy; and the live
PostgreSQL issue saga, which verifies the actual stored key class while retaining rollback,
authorization, restart, retry, and concurrency behavior (1.44 seconds). `target` is 89 GB with
624 GB free, so no cache clean was warranted.

## 2026-08-26 — Teardown measures the write that ended authority

The hosted-agent loop reported token revocation lag by subtracting two copies of the same frozen
task timestamp. Durable teardown happened, but its measured lag was therefore forced to zero and
the bound asserted by the drills could never fail. The revocation store made the contract still
more misleading by accepting and discarding a caller-supplied time. MCP shutdown depended on that
unused time, so a broken wall clock could prevent it from revoking an otherwise live run token.

The revocation provider now owns the whole teardown observation. The production identity adapter
measures the durable store operation with a monotonic clock and reports a conservative whole-second
upper bound; the agent loop records that observed value instead of manufacturing one from workflow
time. The unused timestamp has been removed through the durable store, minter, identity facade,
agent-session cleanup, and MCP router. Governed work still refuses a broken clock, while shutdown
needs only the durable revocation boundary. A regression provider that reports seven seconds proves
the agent telemetry carries a real observation rather than recomputing zero.

**Proof:** all 622 Agent Host, Agent Service, and Identity library stories; all 31 governed MCP
routing stories, including teardown under a failed clock; the focused external run-token, skeleton,
identity spine, and revocation drills; compilation of every Identity test target; strict
all-target/all-feature Clippy for Agent Host, Agent Service, Identity, and MCP; the live PostgreSQL
hosted-agent journey using durable identity, wallet, cost, replay, and teardown state (0.54 seconds);
and both live TypeScript collaboration journeys (36.74 seconds). The latter also exposed a test
whose three bounded asynchronous stages could outlive Vitest's 30-second whole-test ceiling: its
named 60-second story budget now lets the existing 15-second stage bounds produce useful failures.
`target` is 88 GB with 625 GB free, so no cache clean was warranted.

## 2026-08-26 — A timeout exists before its side effect

Flow allowed callers to request timed signal waits and jobs without giving the workflow a durable
timer. A timed wait then computed its deadline from epoch zero and could park forever. A timed job
was more dangerous: it dispatched the external work first and discovered the missing timer only
while arming its deadline. Deadline addition also saturated on overflow, and replay could quietly
turn a timed wait into an untimed one (or the reverse). Two multi-day approval tests omitted timers,
so their prose described durable waiting while their fixtures exercised the broken shortcut.

Workflow timing is now a named context containing the wheel, partition, and current Unix time.
Relative deadlines share one checked calculation. A timed job validates that context and its
deadline before the runner can observe anything; a timed wait validates before journaling or
parking. Signal receipt comparison uses wide arithmetic rather than saturating milliseconds, and
the journaled presence and value of a deadline are replay invariants. Approval and token-remint
stories now advance real durable timers across days instead of relying on an epoch-zero default.

**Proof:** all 242 Flow library stories; the focused multi-day approval and remint drills; strict
all-target/all-feature Flow Clippy; the live PostgreSQL dispatcher story persisting two competing
deadlines and replaying the fired frontier without reopening work (1.14 seconds); and all six live
TypeScript CI lifecycle journeys, covering push and pull-request dispatch, exact-commit sandbox
execution and archived output, failure settlement, and repository isolation (14.94 seconds).
`target` is 82 GB with 630 GB free, so no cache clean was warranted.

## 2026-08-26 — SSH authority has a representable end

Workspace SSH bounded a new grant by the browser capability's signed expiry, but converted an
unrepresentable Unix value to Chrono's maximum date. An unusually large signed expiry could
therefore stop constraining the SSH grant instead of being refused. The handler also read wall time
directly even though the other security boundaries had moved to the shared checked clock.

Capability authentication now rejects every expiry outside the platform's exact RFC 3339 range,
before constructing a request identity. The range ceiling is named once in Events and reused by
structural fixtures instead of representing "long lived" with `i64::MAX`. Workspace SSH retains an
independent defensive conversion, obtains its issuance time from the same checked clock, and
computes one grant deadline as the minimum of five minutes, workspace life, and browser authority.
An invalid or already-ended authority can never be repaired into a live SSH grant.

**Proof:** the exact capability-range and SSH-lifetime stories; all 420 Identity and 365 Edge
library stories; strict all-target/all-feature Clippy for Events, Identity, and Edge; and the live
TypeScript private-work journey, which keeps another human out, grants and replays ephemeral SSH
access, connects through the pinned host key, resumes work from fresh agent context, records the
workspace session, and makes the workspace inaccessible at expiry (8.41 seconds).

## 2026-08-26 — The browser has one session boundary

Edge still carried an older browser-cookie implementation beside the deployed web boundary. It
accepted an in-memory cookie as a credential, exposed refresh and logout routes over that store,
and generated its cookie identifiers from wall-clock nanoseconds plus a process-local counter. No
production path issued those cookies: the web BFF owns the opaque, random, HTTP-only cookie and
presents only a bounded Myelin capability to Edge. Edge login already returned that capability.
The parallel surface was therefore both predictable in isolation and impossible to use as a
coherent lifecycle.

The unused store, cookie parser, public exports, cookie authentication branch, and misleading
refresh/logout routes are gone. Edge now has one human-session boundary: OIDC or browser-approved
device login mints a bounded capability; the web BFF keeps it behind its independently durable
opaque cookie; Edge accepts that capability as Bearer (or stock-Git Basic) and nothing else. A
cookie copied from the browser cannot become an Edge credential.

**Proof:** all 364 Edge library stories; strict all-target/all-feature Edge Clippy; the focused
negative boundary story for copied cookies and retired lifecycle routes; and the live PostgreSQL
TypeScript journey in which a browser approves one short-lived CLI session without transferring
its credential (124 milliseconds).

## 2026-08-26 — An issue action has one durable time

The PostgreSQL Issues store let the database timestamp a row while application code independently
timestamped its event. Relation changes were worse: the issue event and reference-graph event each
read the clock separately. Clock rollback became 1970, and an ordinary user action could therefore
leave three different accounts of when it happened.

The store now acquires one checked clock reading before each state-changing boundary and binds its
Unix form into PostgreSQL while carrying its RFC 3339 form into every co-committed event. Creation
aligns the issue row, pending authorization binding, and request event; activation aligns the
binding and created event; relation changes align the row and both graph projections; closing
aligns the issue state and close event. A clock failure is a typed unavailable response and leaves
neither a row nor an event. The envelope construction and provenance validation also moved into a
small dedicated module, removing that policy from the already-large transaction store.

**Proof:** all 414 Issues library stories; the Edge leak-safe error-mapping story; strict
all-target/all-feature Clippy for Issues and Edge; the live PostgreSQL authorization saga proving
rollback leaves zero state, every row/event timestamp agrees, concurrent retries converge, and
events remain exactly-once (1.41 seconds); and all three TypeScript issue-lifecycle journeys against
the rebuilt system, covering founder defaults, authorization/discovery/close, and dependency
create/retry/remove (11.20 seconds). `target` is 80 GB with 632 GB free, so the useful cache remains.

## 2026-08-26 — One wall clock cannot become several security truths

Authentication, governed MCP work, Git wire credentials, repository grants, agent teardown, CI
leases, and fail-static authorization each had their own wall-clock conversion. Several copies
turned rollback into epoch zero; others silently selected an extreme date or used unchecked
integer casts. Those defaults were individually plausible but collectively dangerous: the same
broken host clock could expire one authority, extend another, and write misleading audit time.

Events now owns one checked wall-clock reading with mutually consistent Unix-second and RFC 3339
forms. Security-sensitive callers either receive that reading or stop before minting a session,
opening a device-login window, issuing Git credentials, routing governed work, changing durable
bootstrap state, claiming a CI lease, or advancing an in-memory merge. One MCP request also reuses
one instant for authorization and completion audit, so a call cannot straddle two contradictory
security truths. Diagnostic-only entropy and display timestamps remain deliberately independent;
they do not grant authority or order durable user work.

**Proof:** rollback and range-boundary stories in Events, Edge, Identity, MCP, Agent Host, and CI;
all library stories in the nine affected crates, including 587 CI control-plane, 365 Edge, 419
Identity, 417 Storage, 177 Substrate, and 34 Agent Host stories; all 30 governed-routing integration
stories; strict all-target/all-feature Clippy; and the complete seventeen-story TypeScript browser
and CLI authentication journey against the rebuilt PostgreSQL-backed system (78.16 seconds).

## 2026-08-26 — One checked Git clock stamps a pull request operation

The durable pull-request store and Edge's in-memory fallback each treated a clock before the Unix
epoch as epoch zero. The PostgreSQL path also cast an unsigned duration to `i64` without checking
the range, formatted its event timestamp through a separate helper, and sometimes read the clock
again while applying the mutation. A broken clock could therefore backdate user-visible Git work
to 1970, while a second boundary could make the record and its event disagree.

Git now exposes one checked clock reading with exact Unix-second and RFC 3339 forms. It rejects
rollback and dates outside the supported year-9999 envelope. Opening, mutating, and finalizing a
durable pull request carry that reading as one operation context, so the record and co-committed
event cannot observe different seconds. Merge and crash-recovery paths acquire the context before
advancing a ref, which keeps a clock failure on the safe side of the irreversible boundary. Edge's
repository edits, review conversations, and ref-event contexts use the same checked source instead
of manufacturing epoch-zero values.

**Proof:** exact clock-boundary and supplied-mutation-time unit stories; the live PostgreSQL PR
boundary, including equal open-record/event timestamps, atomic abort, idempotent commands, merge
finalization, crash recovery, ref-refusal cancellation, and deterministic retry; strict
all-target/all-feature Clippy for Git and Edge; and all twelve stages of the running TypeScript Git
engineering lifecycle against PostgreSQL and the rebuilt Edge (16.85 seconds).

## 2026-08-26 — An advertised tool schema is an enforced product boundary

A caller-graph audit of Agent Service's old plan/apply and HITL surfaces found a complete parallel
product: bespoke effect carriers, capability and delegation traits, in-memory budgets, single and
batch approval loops, card rendering, dry runs, and a second Knowledge agent/gate model. Production
called none of it. Eleven Agent Service drills and two Knowledge contract tests assembled those
copies themselves, while real agent work already flows from Agent Host through MCP governance into
PostgreSQL-backed Storage. Cross-copy parity tests made the duplication look intentional without
testing a user path.

The parallel implementation and its tests are gone. Agent Service's 1,845-line `effect_api` module
is now the small argument-validation boundary production actually calls, and the coverage registry
points only at surviving artifacts. Tool definitions and the shared catalogue remain; durable MCP
governance remains the one place that authorizes, gates, applies, audits, and consumes approval.

That smaller boundary exposed a real bug: MCP advertised closed objects, regex patterns, string
lengths, numeric ranges, and enums but enforced only required fields and primitive types. It now
executes the complete Draft 2020-12 schema with a linear-time regex engine and no HTTP or file
resolver features. The live CLI journey proves unknown fields, empty titles, malformed canonical
references, and over-limit pages are all JSON-RPC invalid-parameter errors before authorization or
mutation.

The new standards validator changed the workspace lockfile, so the offline gVisor Cargo vendor
asset was rebuilt and its lock and canonical-tree identities were intentionally repinned. No
verify-to-use boundary was weakened for the dependency change.

**Proof:** all 160 Agent Service library stories and its surviving integration suite; all 347
Knowledge library stories; all 32 MCP library stories; strict all-target/all-feature Clippy across
the three affected crates; the real 99-row coverage registry; all 18 runner-asset verification
stories; workspace-wide all-target compilation; TypeScript type checking; and the complete
sixteen-stage CLI organization journey against the running PostgreSQL-backed stack (48.13 seconds).
Net removal: 9,179 lines.

## 2026-08-26 — One durable HITL path owns governance

Agent Service exported a second adapter from its in-memory approval model into Storage's durable
gate records. No production caller used it: live governed tool calls enter through MCP's
PostgreSQL-backed verdict store. Its two tests therefore exercised a parallel in-memory assembly,
not the product path, while the adapter itself could panic on clock rollback and silently replace
a serialization failure with an empty risk summary.

The unused persistence adapter, public exports, and self-contained tests are removed. Agent
Service retains the planning and approval vocabulary its callers use; MCP and Storage remain the
single durable implementation for opening, deciding, consuming, and expiring gates. This makes a
green approval test less ambiguous and removes a tempting place to fix the wrong implementation.

**Proof:** no remaining workspace reference to the removed persistence surface; all Agent Service
library stories; strict all-target/all-feature Agent Service Clippy; and workspace-wide all-target
compilation. Net removal: 269 lines.

## 2026-08-26 — The CLI never turns a broken clock into a live credential

Three user-facing authentication checks independently converted a clock before the Unix epoch
into zero: saved browser sessions, device-login responses, and temporary SSH access to an agent
workspace. A broken local clock could therefore make an already-expired credential appear live.
The copies also disagreed about timestamp overflow and made future fixes easy to miss.

The CLI now has one checked wall-clock boundary. Expiring credentials require a representable Unix
timestamp and fail locally when the clock is unavailable; non-expiring externally supplied
credentials do not acquire a new clock dependency. Device login and workspace SSH use the same
rule, and the rollback regression proves that one second before the epoch is an error rather than
1970.

**Proof:** all 155 CLI library stories, strict all-target/all-feature CLI Clippy, the live
browser-approved authentication and private-thread/SSH TypeScript journeys (2 stories in 4.13
seconds), and the full sixteen-story CLI organization journey against the running PostgreSQL,
Git, identity, agent, Chat, Knowledge, Issues, refs, and workspace stack (48.20 seconds), including
saved-session expiry, Git credential refusal, logout, named contexts, governed agent work, and
workspace entry.

## 2026-08-26 — Contract coverage is a registry, not a source-text oracle

The contract-coverage gate inferred test meaning by searching source files for test attributes,
marker comments, contract identifiers, and golden-file path strings. Those checks broke on
semantics-preserving edits and could be satisfied by inert text. Its frontend branch was weaker
still: no real frontend contract was registered, so the green workspace run exercised only a
synthetic fixture.

The gate now does the job it can honestly perform. It reconciles every product-contract row with
an existing coverage artifact or a named deferred landing. Frontend entries require a valid
versioned JSON document with the exact contract identity and at least one vector, plus existing
Rust-provider, frontend-consumer, and browser-journey artifacts. It does not inspect those files
and pretend to know whether they test anything; Cargo, Vitest, Playwright, and the TypeScript
system suite own behavioral execution. The Git and CI read-parity contracts are now registered
against their real shared vectors and all three consumers. The hand-written TOML parser and its
bracket-counting approximation were replaced by the repository's existing Serde/TOML boundary.

The direct JSON dependency changed the workspace lockfile, so the offline gVisor Cargo vendor
asset was rebuilt and both its lock identity and canonical tree digest were intentionally
repinned. This preserves the production verify-to-use boundary rather than bypassing the asset
gate for a development convenience.

**Proof:** 18 lint library stories; the three-case contract-registry executable suite against
both fixtures and the real 99-row manifest; strict all-target/all-feature lint Clippy; all 27 Git
and CI frontend shared-vector stories; the real durable Git Edge shared-vector integration; and
the five-case runner-asset digest suite over the rebuilt 6.4 GB offline vendor tree. The matching
CI Edge integration reached PostgreSQL but its hard-coded development admin credential no longer
matches the running stack, so it is not counted as green.

## 2026-08-26 — An unwired KMS model no longer certifies production resilience

Storage exported a second KMS read path with its own clock, resolved-key cache, fail-static
ladder, signals, and outage drill. No production caller constructed it. Every real encryption
boundary continued to use `KmsEngine` directly, while the drill injected failure into a private
adapter and declared sixteen synthetic reads resilient. The model even contained the same
wall-clock rollback defect fixed in the live authorization caches, but repairing it would only
have strengthened a claim the running product did not earn.

The unused read path, duplicate clock vocabulary, public re-exports, and self-contained outage
drill are removed. The actual KMS engine, durable key hierarchy, encryption boundaries, erasure
paths, and their tests remain. Future remote-KMS resilience must wrap the `KmsAdapter` used by
real product reads and be proven through Edge, not through a parallel test-only stack.

**Proof:** no remaining workspace reference to the removed surface; all 424 Storage library
stories; strict all-target/all-feature Storage Clippy; rebuilt healthy Edge; and both live
TypeScript privacy journeys against PostgreSQL, covering certified agent-data erasure, blocked
reprocessing, private-room preservation, authored Chat erasure, and the retained right to speak
again (5.69 seconds). Net removal: 662 lines.

## 2026-08-26 — Search cache failures neither extend visibility nor retain queries

Search's authorization-filter and ranked-result caches measured age with wall time and
`saturating_sub`. A clock rollback therefore made an old visibility decision or result page look
new again. The result cache had a second failure-path leak: it removed per-query coalescing state
only after a successful DEK seal. During KMS failure or crypto-shred, each distinct user query left
a permanent map entry.

Both caches now use the process-local monotonic clock and one checked freshness rule; an injected
rollback expires and recomputes rather than extending old visibility. Result coalescing is owned
by a scoped guard which removes only its own gate on every exit path, including sealing errors and
unwind, while preserving the successful single-computation behavior for concurrent readers.

**Proof:** rollback stories for both filter and result caches; 32 distinct failed-seal queries
leaving zero in-flight entries; all 17 focused cache stories and all 340 Search library stories;
strict all-target/all-feature Search Clippy; rebuilt healthy Edge; and six live TypeScript code
search journeys against PostgreSQL and stock Git covering exact coordinates, unauthorized
viewers, default-branch movement, feature isolation, merge visibility, and deletion (4.42 seconds).

## 2026-08-26 — Cached authority ages only while the process moves forward

Fail-static authorization measured cache age with Unix wall time and `saturating_sub`. If the
host clock moved backwards, an old allow acquired age zero and could remain usable beyond its
bounded revocation window. Git and Identity authorization, plus control-plane discovery, all
inherited that default through wrappers which advertised a wall clock even though they needed
elapsed time.

The fail-static default is now a process-local monotonic clock. A deliberately injected clock
that moves backwards closes the cache and increments the existing closed signal; it never serves
fresh or stale data from an impossible timeline. The cache-bearing wrappers now state the same
monotonic type, while the separate wall clock remains available to placement and other callers
that produce real timestamps. This keeps elapsed-age policy distinct from timestamp issuance.

**Proof:** the rollback regression and all 177 Substrate library stories; all 190 control-plane
stories; 552 deterministic Git stories; all 419 Identity stories; strict all-target/all-feature
Clippy across Substrate, Control Plane, Git, Identity, and Edge; rebuilt healthy Edge; and the live
TypeScript browser-approved CLI authentication journey against PostgreSQL (133 milliseconds).
The remaining live-Postgres Git library case was not counted: its raw admin login did not match
the database credential after the environment restart.

## 2026-08-26 — Identity never interprets a broken clock as 1970

Seven identity boundaries independently read wall time: OIDC, SAML, WebAuthn, SSH challenges,
PASETO capability verification, machine authentication, and run-token minting. Each converted a
system clock before the Unix epoch into zero; the two timestamp-producing copies then converted
that zero into `1970-01-01T00:00:00Z`. A verifier under that failure could compare credentials
against the distant past, while an issuer could create a lifetime unrelated to real time. The
fallback made a broken security dependency look valid.

Identity now owns one private clock boundary. It checks both the Unix-epoch lower bound and signed
timestamp range, refuses to continue if either invariant is unavailable, and produces RFC 3339
timestamps through the same checked value. All seven production callers use that boundary; their
existing injected test clocks remain unchanged, so protocol tests stay deterministic without a
second production interpretation of time.

**Proof:** the pre-epoch clock regression; all 419 Identity library stories, including OIDC, SAML,
WebAuthn, SSH, capability, revocation, and minting cases; strict all-target/all-feature Identity
Clippy; rebuilt healthy Edge and Workspace Gateway; and the live TypeScript browser-approved CLI
authentication journey against PostgreSQL (121 milliseconds).

## 2026-08-26 — A runner identity is either unpredictable or unavailable

The user-namespace allocator used kernel randomness for its lease nonce but treated its
runner-instance identity differently: if `/dev/urandom` could not be opened or read, it silently
fell back to a process ID and process-local counter. That value participates in deciding whether
an existing namespace lease belongs to this runner. The fallback was predictable, could repeat
after a process restart and PID reuse, and made an entropy failure look like a valid security
identity.

Runner identity creation now has the same fail-closed boundary as lease nonce creation. A complete
128-bit kernel entropy read is required before allocator construction; an unavailable or short
source produces a named `EntropyUnavailable` allocator error, and that result is retained for the
life of the process rather than recovering later under a different identity. The entropy reader is
small and injectable, so the regression story proves both a complete read and refusal of a
15-byte source without inspecting implementation text.

**Proof:** the focused entropy regression; 593 ordinary sandbox stories with 49 explicitly
privileged-host cases skipped; strict all-target/all-feature sandbox Clippy; and the live
TypeScript private-agent workspace journey, which bounded timed-out and overproducing commands,
rejected a conflicting idempotent retry, and then read durable workspace state successfully (4.32
seconds). The all-feature privileged-host lane also refused to pretend this desktop is suitable:
its cgroup delegates memory and process controllers but not CPU.

## 2026-08-26 — A store name is not a privacy capability

Substrate's `HolderRegistry` appeared to implement the deferred automatic
`PersonalDataHolder` contract, but boot only copied `AppSpec.stores` into a second in-memory list.
The architecture check compared those two copies, while a separate H1–H18 enum classified the
same declarations and called the result exhaustive. None of it provided locate, export, rectify,
restrict, or erase, and no product operation consumed the registry. The fixed
`HoldersSpec::Auto` field had no alternative behavior. Service tests therefore certified privacy
coverage by repeating that each service had a datastore.

The registry, duplicate manifest, classifier, inert application fields, and self-certifying tests
are gone. The neutral `StoreKind` taxonomy remains because real Search and References residency
descriptors use it. The cross-language contract name and coverage row also remain: contract 1.4
is still explicitly deferred to `P-GDPR-DURABLE-HOLDER-INVENTORY`, where a future implementation
must register executable durable holders rather than metadata. The workspace check also found a
stale Identity cell-scale drill importing Storage's deleted process-local restore model; its
hand-written RPO/RTO durations and signals are gone, while the actual authority-resurrection and
cell-bulkhead cases and the restore harness's timing drills remain.

**Proof:** workspace all-target/all-feature compile and strict Clippy; the 99-row contract gate;
all 176 Substrate library stories and every lifecycle, topology, migration, firehose, load, IDOR,
and restore drill; all three full-stack Identity cell-scale cases; the Agent, Control Plane, Flow,
Knowledge, and Search library suites (including Search's two independent 3,000-event freshness
measurements); rebuilt healthy Edge; and the public browser-to-CLI authentication journey (398
milliseconds). Net removal before this entry: 1,334 lines of declarative privacy simulation and
fabricated recovery evidence.

## 2026-08-26 — Restore erasure is proven against the database it protects

Storage still contained a second privacy stack after the legacy service was removed. A generic
eraser accepted caller-supplied pseudonym, Search, References, Bus, Git, and ledger doubles; an
eighteen-entry holder list then marked every remaining store reached by iterating that same list.
The Git leg destroyed a blob key in the in-memory KMS and claimed reflogs, bitmaps, and backups were
covered by returning their enum variants. Multi-cell fan-out silently omitted failed cells, while
post-restore tests rebuilt hand-authored vectors and replayed through the same process-local state.
None of these types had a caller outside their own unit and drill suites.

That parallel eraser, declared holder inventory, Git reach model, multi-cell twin, granularity
posture, synchronous re-erasure bridge, and their endorsing tests are gone. The durable post-PIT
ledger now owns its record type and exposes only explicit asynchronous product scopes. Edge still
wires independent agent-data and Chat post-restore operators, while the restore gate refuses a
post-backup erasure unless that real operator path has dealt with it. The retained integration
story uses `pg_dump` and `pg_restore`, first proves the erased key was genuinely resurrected, then
proves the durable holder destroys it and restores its absorbing erasure marker.

The full run also exposed relay-test contention with the live product: schema-isolated tests shared
PostgreSQL's database-global production election key and could nondeterministically become standby.
Production construction still uses the one shared key; test-support construction can now provide
an isolated election key, so the concurrency, outage, quarantine, and recovery stories test their
own relay rather than racing Edge.

**Proof:** all-target/all-feature Storage compile and strict Clippy; the 99-row contract gate; all
436 remaining Storage library stories and the complete PostgreSQL, Valkey, object-store, sandbox,
relay, durable privacy, and real backup/restore suite; rebuilt Edge; and both public TypeScript
privacy journeys (5.34 seconds). Net removal before this entry: 5,953 lines of in-memory privacy
twins and self-certifying drills.

## 2026-08-26 — Privacy compliance is a product path, not a parallel simulation

`myelin-gdpr-service` looked like the system's privacy control plane, but it had no binary and no
production consumer. Its only dependants were dev-dependencies used by tests. Behind the broad API
were process-local holder inventories, erasure ledgers, retention state, audit logs, eDiscovery
exports, agent traces, and service-specific doubles. Their tests assembled those models directly,
then certified the state they had just arranged; a running privacy request could reach none of it.
Search also published a constants-only posture whose complete conjunction was true by declaration.

The legacy service, its external endorsing tests, and Search's declared-green posture are gone.
The small `myelin-gdpr` contract crate remains, as do the paths that change durable product state:
Storage's leased privacy requests and encrypted agent traces, Chat's authored-message erasure,
Search's real eraser, cross-cell fan-out, and restore re-erasure. Removing the workspace member
changed the lockfile, so the full offline Cargo vendor asset was deliberately rebuilt and repinned
by both lockfile and canonical-tree digests.

The contract manifest now distinguishes proof from aspiration. All 99 rows reconcile to existing
tests or a named landing prompt: 75 are covered and 24 are explicitly deferred, with zero invented
files. Durable holder inventory, data mapping, notification erasure, audit, eDiscovery, policy
controls, and a system-wide immutable-content posture remain visible gaps rather than capabilities
implied by an unwired crate.

**Proof:** workspace all-target/all-feature compile and strict Clippy; the contract-coverage gate;
all five vendored-runner asset guards; the complete Control Plane, Issues, Search, and Storage
suites against PostgreSQL, Valkey, object storage, and sandbox drills; rebuilt Edge; and both live
TypeScript privacy journeys (5.07 seconds), covering a resumable request, reconstruction refusal,
Chat erasure, and the person's right to speak again. Net removal: 25,570 lines of parallel privacy
simulation and self-certifying tests before this worklog entry.

## 2026-08-26 — Opening a store no longer creates a pretend DSR capability

Storage supplied generic OLTP, blob, and OLAP "holders" whose registration method merely returned
their name and changed no registry. Every privacy method failed with a note pointing at future
work. Identity then carried one of these objects in five stores solely so unit tests could call
that no-op registration, while Substrate independently maintained the real boot-time store
manifest. OLAP combined its placeholder with a green signal whose `holder_registered` input was a
literal `true` in the same test.

The generic holders and their failing DSR surfaces are gone, as are the unused Identity fields,
constructors, accessors, and self-certifying tests. Substrate's actual store manifest and registry
remain the one boot inventory for OLTP, blob, cache, and search stores. The OLAP projection keeps
its event ingestion, regional boundary, restriction behavior, and source-reindex parity without
claiming a privacy capability it does not have.

**Proof:** all-target/all-feature compile and strict Clippy across Storage, Substrate, and Identity;
all 502 Storage, 192 Substrate, and 419 Identity library stories plus their complete PostgreSQL,
Valkey, object-store, restore, and sandbox-backed suites; rebuilt Edge; and the live TypeScript
browser-to-CLI authentication journey (253 ms). Net removal: 404 lines of placeholder holder
objects and self-certifying tests.

## 2026-08-26 — Knowledge privacy has one durable agent-trace implementation

Knowledge exposed a privacy holder backed by a process-local restriction set. Its locate,
rectify, and ordinary export methods made content-addressed receipts without reading or changing
Knowledge rows; its erase method made another receipt without invoking the separate eraser. That
eraser destroyed keys in Storage's in-memory KMS and purged caller-supplied IDs from in-memory
Search and References models. An adjacent agent-trace holder stored traces in a local map and
claimed backup-safe erasure through the same model.

Those duplicate holders, synthetic personal-data schema, in-memory erasure floor, and endorsing
suites are gone. Knowledge page encryption, the PostgreSQL page store, collaboration transport,
and Search feed remain. Agent traces have one product implementation: Storage's PostgreSQL-backed
`DurableAgentTraceStore`, composed by Edge for trigger history, privacy requests, and restore
re-erasure. Knowledge itself remains absent from privacy-request scopes until its durable pages,
blocks, comments, references, and search projections can be handled as one resumable operation.

**Proof:** all-target/all-feature Knowledge compile and strict Clippy; all 365 remaining library
stories and the complete PostgreSQL/object-store-backed crate suite; rebuilt Edge; and both live
TypeScript Knowledge journeys covering durable create/edit/conflict handling and a living document
that adds then forgets an Issue reference (3.89 seconds). Net removal: 2,252 lines of duplicate
privacy models and self-certifying tests.

## 2026-08-26 — Control Plane privacy inventory follows the schema, not a second list

Control Plane exported a zero-sized privacy holder whose locate and export operations returned
fixed successful receipts without consulting the registry, while its destructive operations only
explained that the registry should contain no personal data. Its supporting inventory was a
handwritten list of columns, tested only against itself; adding a real schema column could not make
the test fail. No running privacy request constructed the holder.

That ceremonial holder, the independently maintained column list, and the production GDPR
dependency used only by them are gone. The durable registry schema remains the source of truth,
while the real cross-cell DSR fan-out and storage re-erasure boundary retain their behavioral
tests. If personal data enters the registry, it must arrive with a durable operation over that data
rather than a receipt generator that says the intended architecture is safe.

**Proof:** all-target/all-feature Control Plane compile and strict Clippy; all 191 remaining
library stories and the complete PostgreSQL-backed crate suite; rebuilt Edge; and the live
TypeScript first-hour journey from one browser approval through project, repository, issue, CI,
and governed-agent setup (26.93 seconds). Net removal: 183 lines of ceremonial inventory and tests.

## 2026-08-26 — Git privacy no longer counts wired doubles as product holders

Git's `PersonalDataHolder` returned successful locate, export, rectify, and restrict receipts
without reading or changing Git state, while its trait erase correctly refused to claim success.
A separate "real fan-out" existed only in tests: process-local KMS state and eight seam doubles were
invoked, after which all eight holders were marked reached by construction. The durable Git stores,
PostgreSQL pull-request rows, object packs, Search, References, Bus, and the production privacy
request path never composed it. Ref stores also carried a boolean holder-registration field that no
runtime behavior read.

The ceremonial holder, registration object, and endorsing suites are gone; the real sandboxed
history-rewrite tool remains independently tested. During the full regression run, a source-scanning
unit test failed because it required every path in the Git index to exist in the dirty worktree.
That test also enforced secret policy by string-scanning Myelin's own checkout instead of exercising
the push boundary. It is gone; the actual quarantine tests still prove that detected credentials
are rejected before object promotion or ref movement.

**Proof:** all-target/all-feature Git compile and strict clippy; all 553 remaining library stories;
the complete all-feature suite against PostgreSQL, Valkey, and on-disk repositories, including
external `git fsck` and destructive backup/restore; rebuilt Edge; and all twelve live TypeScript Git
journeys from concurrent repository creation through editing, review, branch protection, and merge
(13.28 seconds). Removed 1,560 lines of unshipped holder scaffolding and brittle source inspection.

## 2026-08-26 — References privacy claims now stop at the durable boundary

References exposed two privacy holders backed by its process-local edge projection. One returned
successful structural-erasure receipts while preserving every edge; the other could evict a real
Valkey entry, but discovered affected keys from that in-memory twin rather than the production
`PgEdgeStore`. Neither holder was constructed by a running privacy request. A public restore driver
then simulated backup recovery by manually repopulating local maps and an in-memory KMS, and called
its own second pass restore-safe. A constants-only posture test completed the appearance of shipped
coverage without observing any product state.

The holders, suppression set, simulated erasure ledger and restore driver, posture declaration,
and endorsing suites are gone. The edge and cache store names now live in a neutral three-line
module used by residency metadata. The real reference graph remains: PostgreSQL ingestion and
tombstoning, encrypted Valkey caching and invalidation, leak-free resolution, and reindex parity
all retain their own direct tests. References remains absent from privacy-request scopes until a
durable operation locates actor edges in PostgreSQL, purges derived cache entries, records its
obligation, and proves re-erasure after an actual backend restore.

**Proof:** all-target/all-feature References compile and strict clippy; all 275 remaining library
stories; the complete all-feature suite against PostgreSQL and Valkey; rebuilt References and Edge;
and all three live TypeScript graph journeys covering lossless paged backlinks, outbound traversal,
and indistinguishable 404s for a private artifact (14.59 seconds). Removed 2,039 lines of simulated
privacy machinery and self-certifying tests.

## 2026-08-26 — Workflow privacy no longer presents a process-local prototype as durable

Workflow exposed an optional `PersonalDataHolder` whose ordinary construction returned successful
no-op privacy receipts and whose "backed" construction read only the process-local workflow
journal. A special constructor made the story look stronger by encrypting synthetic history under
Storage's in-memory KMS, destroying that in-memory key, and inspecting the same process's snapshot.
No running service constructed any variant, no PostgreSQL workflow row was read or changed, and no
privacy request could reach it. Its restore-safety claims therefore certified the prototype rather
than the product.

The holder, crypto-shred prototype, public exports, classification endorsement, and both dedicated
suites are gone. Workflow's real service inventory remains automatically registered at boot, and
the durable PostgreSQL engine, replay journal, signal buffer, timer wheel, and lease-fenced worker
are unchanged. Workflow remains honestly absent from privacy-request scopes until its durable
history can be located and erased through the same production path, with persistent key state and
post-restore replay evidence.

**Proof:** all-target/all-feature Workflow compile and strict clippy; all 236 remaining library
stories; the complete all-feature suite against PostgreSQL, including 100k timers, durable replay,
lease fencing, exact-once signals, and CI wake-ups; rebuilt Hosted Agent Worker, CI Dispatch, and
Edge; and both live TypeScript private-agent journeys, including a named private problem and
workspace resumed from fresh agent context (6.63 seconds). Net removal: 1,475 lines of unshipped
privacy prototype and endorsing tests.

## 2026-08-26 — Notifications no longer certify an in-memory erasure

Notifications exported a privacy holder whose default instance returned successful locate,
export, rectify, restrict, and erase receipts without any backing store. Its backed variant counted
rows in the process-local inbox projection but called leaving every row unchanged a successful
"structural erase." The accompanying PostgreSQL test did not invoke an eraser at all: it selected
the same rows twice and treated their equality as proof. A second test-only eraser destroyed keys,
recorded restrictions, and wrote an erasure ledger only in local maps while its tests described the
result as durable and restore-safe.

Those two simulated privacy paths, their public exports, and their self-certifying suites are gone.
The useful behavior hidden in the broad drill remains as a narrow story: once Identity resolution
reports an actor erased, every notification appearance renders as an unlinkable erased-user
tombstone. The real inbox, preference, delivery, provider, and replay paths remain intact.
Notifications now stays honestly absent from privacy-request scopes until one durable operation can
discover inbox and off-cell delivery residuals, apply a durable routing restriction, destroy real
keys, request provider erasure, record the proof, and resume after interruption or restore.

**Proof:** all-target/all-feature Notification compile and strict clippy; all 330 remaining library
stories; the complete all-feature suite against PostgreSQL; rebuilt Notifications and Edge; and all
five live TypeScript notification journeys covering durable delivery, deduplication, coalescing,
tenant visibility, read/snooze/resurface state, completion, self-suppression, and cursor traversal
(109.41 seconds). Net removal: 1,751 lines of in-memory compliance models and endorsing tests.

## 2026-08-26 — CI no longer reports an erasure it did not perform

CI exposed a `PersonalDataHolder` whose ordinary `erase` returned success without calling its
destructive fan-out. The separate fan-out destroyed keys and tombstoned artifacts only in local
memory. Its PostgreSQL integration test then manually issued an `UPDATE` to pseudonymise the real
row after the model had already reported green, so the test made the database resemble the receipt
instead of proving that the receipt's operation changed the database.

The holder, in-memory fan-out, public exports, and both endorsing suites are gone. The erased-actor
marker was the one production dependency hidden in that model; it now lives beside the durable CI
run store that accepts pre-erasure replays after an actor edge has actually been pseudonymised. CI
remains honestly absent from privacy-request scopes until one durable operation can discover its
PostgreSQL rows and object-store logs, destroy the right keys, pseudonymise the rows, record a
post-restore obligation, and prove the complete result.

**Proof:** all-target/all-feature CI Control Plane compile and strict clippy; all 586 remaining
library stories; the complete all-feature suite against PostgreSQL and real sandbox execution,
including the 118.19-second definition-cutover and 135.66-second terminal-accounting race suites;
rebuilt CI Control Plane and Edge; and all six live TypeScript CI delivery journeys, including exact
commit execution, persisted sandbox output, failure inspection, and repository visibility
(11.71 seconds). Net removal: 1,502 lines of unshipped privacy models and self-certifying tests.

## 2026-08-26 — Search privacy reaches the index it describes

Search exported a zero-sized `SearchIndexHolder` beside its working `SearchEraseHolder`. The former
returned successful locate, restrict, and erase receipts whose outcomes said `stub` and `no-op`;
the latter locates subject facets, sends removal events through the live indexer, compacts vectors,
and participates in restore verification. Only the stub and its own contract test used the false
type, while the deployable shell already inventories its index through `AppSpec`.

The stub holder and its self-certifying suite are gone. The index store identity now lives in a
small neutral module shared by layout, residency, the service shell, and the real eraser. The same
audit found that real erasure events carried hard-coded epoch-zero timestamps. Their clock is now
an explicit dependency with a production UTC implementation and a deterministic test clock, so an
erasure records when it happened without making the rest of the holder harder to test.

**Proof:** all-target/all-feature Search compile and strict clippy; all 339 Search library stories;
the complete all-feature Search suite, including PostgreSQL bootstrap, live-consumer erasure,
restore re-erasure, object-store backstop, cross-tenant isolation, and the 43.58-second freshness-at-
scale drill; rebuilt Edge; and all six live TypeScript code-search journeys from default-branch
indexing through stale-match and deletion cleanup (3.45 seconds). Removed 341 lines of no-op holder
code and endorsing tests.

## 2026-08-26 — Agent privacy receipts describe durable work

Agent Service exported three parallel privacy models while Edge constructed only one. Two public
registration holders returned successful locate, export, restrict, and erase receipts without
reading or changing a store—the erase outcome literally said `no-op`. A larger in-memory fabric
model tombstoned vectors, destroyed process-local keys, and replayed a private in-memory ledger, but
was likewise used only by its own tests. Production privacy requests, durable traces, restrictions,
erasure proofs, and post-restore replay all use Storage's PostgreSQL-backed
`DurableAgentTraceStore` instead.

Both ceremonial models, their public exports, and the suites that certified them are gone. The one
useful Knowledge tool drill no longer invokes a no-op trace holder as evidence of erasure. During
the regression run, a hard-coded platform-tool count also failed after a valid tool was added; that
incidental count is gone while the catalogue's sorted uniqueness, validation, cross-subsystem
presence, latest-version projection, and approval-contract behavior remain asserted.

**Proof:** all-target/all-feature Agent Service compile and strict clippy; all 226 remaining library
stories; the complete all-feature Agent Service suite against PostgreSQL, including durable RLS,
tool-scope, trigger-handoff, and wallet-metering journeys; rebuilt Agent Service and Edge; both live
TypeScript privacy journeys (5.33 seconds); and both private-agent-thread journeys, including
fresh-context continuation in a private persistent workspace (7.22 seconds). Net removal: 1,481
lines of unshipped models and self-certifying tests.

## 2026-08-26 — Chat privacy has one truthful implementation

Chat exposed two incompatible erasure stories. The production privacy request path uses the
PostgreSQL-backed message holder, durable key hierarchy, durable erasure ledger, and restore
re-erasure worker. Beside it, an unconstructed in-memory cascade claimed to erase messages, drafts,
Identity, read state, Search, Notifications, and analytics by changing local maps, flags, and
counters. Its receipt declared complete coverage even though none of those product stores had been
reached. Two suites then certified the simulation rather than a user-visible operation.

The ceremonial cascade, holder, public exports, and self-certifying suites are gone. The real
durable holder remains the sole Chat participant in privacy requests. The analytics restriction
flag now lives beside its gate, and Search and the durable eraser share the canonical
`chat.message.erased` event directly rather than reaching through the deleted model. Existing
message encryption, read-state, restriction, and search stories now test their own behavior without
borrowing credibility from a fictional all-holders receipt.

**Proof:** all-target/all-feature Chat compile and strict clippy; all 262 remaining Chat library
stories; the complete all-feature Chat suite against PostgreSQL, including the 37.07-second real
backup/restore re-erasure story; rebuilt Edge; both live TypeScript privacy journeys (5.34 seconds);
and all twelve live Chat collaboration journeys, including private rooms, reply threads, private
agent work, and persistent workspace resumption (80.79 seconds). Net removal: 1,808 lines of
simulated product code and endorsing tests.

## 2026-08-26 — Issues privacy coverage no longer certifies a simulation

Issues exported a public-looking privacy holder and erase fan-out that no service constructed. It
used a private in-memory ledger, destroyed keys in a process-local KMS, represented Identity,
Search, Refs, and OLAP effects by counters or flags, and still returned a receipt claiming every
holder had been reached. Two Rust suites exercised and endorsed only that twin. This made an honest
missing product slice look shipped and made the future durable design harder to see.

The ceremonial holder, simulated fan-out, private ledger, public exports, and endorsing suites are
gone. The real Issues personal-data inventory remains, and the analytics restriction primitive now
lives beside the projection that consumes it. Issues deliberately remains absent from privacy
request scopes until its PostgreSQL records, durable key hierarchy, derivative services, and
post-restore obligation can participate in one truthful operation.

**Proof:** all-target/all-feature Issues compile and strict clippy; all 414 remaining Issues library
stories; the complete all-feature Issues suite against the configured PostgreSQL backend, including
the million-row bounded-query and durable authorization/restart stories; rebuilt Edge; and all three
live TypeScript Issues lifecycle journeys (10.21 seconds). Net removal: 1,632 lines of simulated
product code and self-certifying tests.

## 2026-08-26 — authored Chat erasure survives a real database restore

Chat re-erasure was production-wired but its restore test rebuilt the expected rows by hand. That
proved holder selection and idempotency, not that a real PostgreSQL backup could resurrect the
encrypted body and key material or that the shipped restore path would remove them again.

The new integration story takes a custom-format backup containing two people's encrypted messages,
erases one person through the production holder, and restores the pre-erasure database. It first
proves the old private body is decryptable again—the drill has teeth—then drives the live ledger
through the production restore holder. The old body becomes unrecoverable and its row a tombstone,
the neighboring person's message remains readable, and a response-lost replay preserves new Chat
history the erased person deliberately authored after recovery. This keeps Chat-history erasure
narrow without weakening restore safety.

**Proof:** strict all-target/all-feature Chat clippy; all 275 Chat library stories; all six existing
real-PostgreSQL durable Chat erasure cases (1.91 seconds); the new real `pg_dump`/`pg_restore` story
(37.82 seconds); and both live TypeScript privacy journeys, including the right to speak again after
authored-history erasure (5.09 seconds).

## 2026-08-26 — a poison batch cannot strand healthy events

`drain_to_empty` stopped whenever a pass published or deduplicated nothing. Moving a poison event
to the dead-letter queue was real forward progress, but the aggregate receipt both discarded that
count and treated the pass as stalled. A full bounded batch reaching its retry ceiling could
therefore hide every healthy event ordered behind it while reporting an incomplete quarantine.

Drain aggregation now has one small seam that owns both progress and receipt accounting. Dead-
lettering frees queue capacity, continues the drain, and remains visible to the operator. The
durable story stages 257 ordered events, exhausts all 256 rows in the first batch, then proves the
healthy final event is delivered exactly once and the live queue is genuinely empty.

**Proof:** strict all-target/all-feature clippy for Events and Storage; all 217 Events library
stories; the real-PostgreSQL full-poison-batch regression (0.41 seconds); restarted the outbox
publisher; and the live TypeScript first-hour organization journey from browser approval through
project, repository, issue, CI, and governed-agent work (18.98 seconds).

## 2026-08-26 — an outbox outage sheds readiness without killing the service

Every service tick refreshed outbox telemetry through infallible PostgreSQL reads. Losing the
database could therefore panic a healthy process while it was merely measuring queue depth; relay
drain and projection rebuilds carried the same false assumption. The fallback was especially
misleading at an operational boundary: either the process disappeared or unknown state looked
like ordinary state.

Durable outbox reads now have an explicit fallible vocabulary used throughout service telemetry,
relay drain, bus signals, and the Events, Notifications, Refs, and Search rebuilders. A failed
telemetry refresh preserves the last trustworthy measurements, marks a distinct critical outbox
dependency down, and lets the service loop live so readiness can recover when storage returns.
Boot and graceful drain still fail loudly because they cannot honestly complete without reading
the queue. Snapshot size failures remain typed and distinct from storage unavailability.

**Proof:** warning-free workspace all-target/all-feature compile and strict clippy across Events,
Storage, Substrate, Notifications, Search, and Refs; all 216 Events and 192 Substrate library
stories; the relevant Notifications, Search, and Refs rebuild suites; all nine real-PostgreSQL
durable-outbox journeys, including a closed-pool outage, non-panicking relay, bounded snapshot
errors, and exactly-once recovery (1.45 seconds); restarted Edge, Agent Service, Hosted Agent
Worker, and Workspace Gateway; and both live TypeScript private-agent-thread journeys, including
fresh-context continuation in its persistent workspace (7.55 seconds).

## 2026-08-26 — an unreadable dead-letter queue is not an empty queue

The PostgreSQL consumer dead-letter adapter logged read failures and returned an empty list. That
made a storage outage indistinguishable from a healthy queue with no quarantined events—the exact
moment an operator most needs an honest answer. Writes also exposed arbitrary database strings
through an otherwise deliberately small durability boundary.

Dead-letter reads and writes now share a typed, payload-free availability error. Failed writes
still retain an emergency in-process copy and withhold broker acknowledgement; failed reads now
reach the caller instead of manufacturing an empty queue. The test vocabulary follows the user
story directly: after a durable quarantine reconnects, the poison is visible; while its database
is unreachable, the local copy remains visible and the durable view explicitly says unavailable.

**Proof:** warning-free all-target/all-feature compile and strict clippy across Events, Storage,
and Substrate; all 216 Events library cases; and all three real-PostgreSQL durable dead-letter
journeys, including idempotent restart recovery, payload-free panic quarantine, and unavailable
storage that withholds acknowledgement and never reports a false empty queue; restarted
Notifications; and all five live TypeScript notification-lifecycle journeys (107.06 seconds
total).

## 2026-08-26 — notification rebuild receipts describe applied work

Notification reindexing counted every delivery other than a duplicate as successfully replayed.
That included quarantined snapshots, scheduled retries, unavailable dependencies, and tenant
throttling, so an operator could receive a complete-looking receipt while the rebuilt inbox was
missing rows.

The rebuild now distinguishes applied, deduplicated, and incomplete delivery dispositions. Any
incomplete snapshot returns a typed, payload-free error naming the deterministic event ID and safe
state; no replay count or success receipt is produced. A full rebuild also has an explicit test for
unavailable dedup storage, ensuring it cannot apply snapshots against unknown reset state.

**Proof:** strict all-target/all-feature Notifications clippy; all 17 notification reindex unit
stories and the notification replay contract/drill cases selected by that suite, including a
quarantined cross-tenant snapshot that leaves the inbox empty and remains operator-visible;
restarted Notifications; and all five live TypeScript notification-lifecycle journeys (107.42
seconds total).

## 2026-08-26 — bus erasure survives a ledger outage without panicking

The bus-erasure ledger still panicked on PostgreSQL write, point-read, and replay-set failures. A
write outage could unwind the request after its key had already been irreversibly destroyed, while
a read outage could crash the post-restore sweep instead of withholding its green receipt.

The ledger now has one typed availability result across all operations. Live erasure and restore
re-erasure return a composite error that distinguishes key destruction from ledger availability,
so callers can retry interrupted work without confusing either failure. An irreversible key step
may still precede the durable record, but no receipt is minted until that record exists; the same
request is idempotent and completes it after recovery. An unreadable restore obligation likewise
produces no success receipt and is re-applied by a fresh worker.

**Proof:** a warning-free workspace all-target/all-feature compile; strict all-target/all-feature
clippy for Events and Storage; all 231 Events library cases; and all five real-PostgreSQL durable
bus-erasure journeys, including closed-pool failure of every ledger operation, retry after the
irreversible live key step, and retry of a restore sweep that finishes with zero resurrected keys
(0.17 seconds total); restarted affected services; and both live TypeScript privacy-lifecycle
journeys (5.16 seconds total).

## 2026-08-26 — event delivery waits for durable dedup storage

The PostgreSQL dedup adapter translated every storage failure into an ordinary boolean. Most
dangerously, failed co-commit acquisition returned a fresh, no-op transaction whose commit always
succeeded. A consumer could therefore run a handler during a database outage and acknowledge the
delivery without durably recording either its dedup mark or its transactional effect.

Dedup reads and mutations now return a small typed availability result. Co-commit acquisition has
no fallback transaction: an outage leaves the delivery pending and returns the standard retry
outcome before the handler can run. Non-retryable delivery also waits until its post-quarantine
dedup mark is durable before acknowledging, and full notification rebuilds surface a failed dedup
reset instead of quietly replaying against stale state.

**Proof:** a warning-free workspace all-target/all-feature compile; strict clippy across Events,
Storage, and Notifications; all 231 Events library cases; three real-PostgreSQL co-commit journeys,
including closed-pool failure for every dedup operation followed by a same-delivery recovery that
commits one mark and one effect (0.17 seconds total); restarted affected services; and both live
TypeScript private-agent-thread journeys (7.44 seconds total).

## 2026-08-26 — token mint waits for its durable run grant

Run-token minting recorded the credential lifetime but discarded the result of writing the
matching expiring relationship into the authorization graph. A graph outage could therefore return
a signed, live credential whose `run#run_bound` edge never existed, leaving downstream behavior to
depend on which of the two security records it consulted.

Minting now has two explicit durable prerequisites: its lifetime record and its run grant. Either
failure withholds the credential and reaches the caller as a storage failure. Both writes remain
idempotent, so a caller can retry the same deterministic mint request after recovery without
creating a wider or longer-lived credential.

**Proof:** all 424 identity library cases; warning-free all-target/all-feature Identity clippy; all
five real-PostgreSQL durable-revocation/mint journeys, including independently healthy lifetime
storage and unavailable tuple storage followed by a successful same-request retry (0.21 seconds
total); restarted Edge; and both live TypeScript private-agent-thread journeys (7.21 seconds total).

## 2026-08-26 — durable revocation failures reach the caller

The revocation store still used process panics for denylist and run-teardown writes, while its
diagnostic count translated a database outage into an empty denylist. That was especially unsafe
after irreversible work: pseudonym erasure could destroy a subject key and then panic while
disabling the principal, and token minting could hand control to an infallible persistence call.

Every revocation mutation now returns its durable result, and the shorter panic wrappers are gone.
Token minting refuses to return credential material unless its run lifetime was recorded. Erasure
and restore replay withhold their receipts until principal disablement succeeds. Agent-session,
agent-host, operator, and MCP shutdown boundaries either propagate the failure or terminate with a
clear error; failed writes do not increment success telemetry. The diagnostic count is fallible too,
so unavailable storage can no longer resemble a legitimately empty tenant partition.

**Proof:** all 424 identity library cases; warning-free strict clippy across Identity, Agent Host,
MCP, Edge, and CI Control Plane; a workspace-wide all-target/all-feature compile; all four
real-PostgreSQL durable-revocation journeys, including closed-pool failure of every mutation and a
fresh-provider retry (0.20 seconds total); restarted affected services; and both live TypeScript
private-agent-thread journeys, including fresh-context resumption and OpenSSH workspace access
(7.52 seconds total).

## 2026-08-26 — interrupted pseudonym erasure resumes without a process panic

Pseudonym-map deletion and the restore-replay ledger still treated PostgreSQL failures as reasons to
panic. Their neighboring reads also offered shorter panic wrappers beside fallible methods, making
an outage easy to mistake for an absent mapping, an empty tenant, or no erasure obligation.

Every pseudonym and ledger operation is now explicitly fallible, including row shredding and ledger
recording. `erase_in` propagates either failure and cannot mint a receipt until both durable steps
finish. The irreversible key step remains safely retryable: if key destruction succeeds and the
database then disappears, a fresh worker observes the already-destroyed key, deletes the surviving
mapping, records the replay obligation, and only then returns the truthful receipt. The obsolete
panic-helper test is gone; the production PostgreSQL adapter itself supplies the evidence.

**Proof:** all 424 identity library cases; warning-free all-target/all-feature Identity clippy; and
all four real-PostgreSQL pseudonym journeys, including a closed-pool interruption followed by a
fresh-provider retry that completes the row shred and durable ledger record (0.35 seconds total).

## 2026-08-26 — principal-directory reads cannot panic or masquerade as absence

The principal store exposed fallible `try_*` reads beside convenience methods that panicked on the
same PostgreSQL and corrupt-row failures. That doubled the API, made the unsafe spelling shorter,
and left every new caller one easy choice away from crashing an authentication or privacy path.

There is now one unsurprising API per read: point lookup, tenant scan, profile-erasure key lookup,
and credential resolution all return `Result`, with absence represented only inside the successful
value. Production callers, integration tools, and tests use those same names and must decide how an
outage reaches their boundary. The obsolete panic wrappers and their `try_` twins are gone.

**Proof:** all 425 identity library cases; warning-free all-target/all-feature Identity and Edge
clippy; all eight real-PostgreSQL durable-identity journeys, with one closed pool proving that all
four reads return storage errors rather than absence or a panic; restarted Edge; and the two live
private-agent-thread journeys, including fresh-context resumption and real OpenSSH workspace access
(7.82 seconds total).

## 2026-08-26 — SSH authentication fails closed when its identity directory is down

The durable SSH key adapter was the lone identity authenticator still using a legacy convenience
read that panicked on PostgreSQL errors. A transient directory outage could therefore unwind an SSH
admission path instead of producing an ordinary denial, despite every later principal lookup being
fallible.

Key-binding resolution is now explicitly fallible. The in-memory directory remains infallible, the
durable adapter translates storage and corrupt-row failures into a credential-free typed error, and
the verifier returns `Unavailable` before consuming the one-shot signed challenge. That ordering
lets a person retry the exact request after the directory recovers without weakening replay
protection after a binding has actually been resolved.

**Proof:** all 425 identity library cases; warning-free all-target/all-feature Identity clippy; a
recovery story in which the first verification is denied and the same challenge subsequently
succeeds; and all eight real-PostgreSQL durable-identity journeys, including a closed-pool lookup
returning `Unavailable` without a panic.

## 2026-08-26 — privacy holder work keeps its fenced request lease alive

Privacy requests acquired a 120-second lease and then performed all holder work without renewing
it. Bounded Chat erasure removed the dangerous large transaction, but also made the mismatch plain:
a sufficiently large history could make correct durable progress past the lease, then lose the
authority needed to publish its certificate.

The privacy-request store now offers a fenced heartbeat. It extends only a still-live processing
lease with the same owner and epoch, never shortens an existing expiry, cannot resurrect an expired
worker, and cannot reopen a completed request. Edge polls holder work alongside that heartbeat every
40 seconds. A lost lease cancels the current future and returns the current durable request instead
of allowing stale completion; Chat's already-committed batches remain available to the next owner.
Transient holder failures and violated proof invariants remain distinct public/internal outcomes.

**Proof:** warning-free all-target/all-feature Storage and Edge clippy; all 508 Storage and 363 Edge
library cases; all three real-PostgreSQL privacy-request journeys, including a two-second lease kept
authoritative past its original expiry and terminal completion refusing a later heartbeat; restarted
Edge; and both live TypeScript privacy lifecycle journeys (5.28 seconds total).

## 2026-08-26 — authored Chat erasure makes bounded, resumable progress

Authored-message erasure previously loaded, locked, tombstoned, and emitted consequences for a
person's entire Chat history in one PostgreSQL transaction. The key-destruction boundary was sound,
but transaction duration and memory grew without bound; a large account could also lose all
database progress to one late outbox failure.

The durable operation now advances through two bounded phases under the existing author fence.
Envelope verification records a monotone message cursor before the independent Chat key can be
destroyed. Mutation then tombstones at most 100 messages at a time, co-commits exactly 100 or fewer
erasure events, and advances equal cumulative counts in that same transaction. A retry starts with
the remaining live row, while the final certificate is impossible until no remainder exists and
the operation's cumulative counts become its immutable receipt. A partial index serves both bounded
walks. The one-way database guard permits only forward cursor/count transitions and completion.

The upgrade preserves historical evidence: completed pre-batching operations are backfilled from
their immutable final receipts while their old trigger is deliberately replaced; pending operations
remain unverified and must traverse the new proof path. The production accumulated database exposed
that compatibility requirement during restart, and a dedicated real-PostgreSQL story now keeps it
from regressing.

**Proof:** warning-free all-target/all-feature Chat clippy; all 275 Chat library cases; all six
real-PostgreSQL Chat erasure journeys, including a second-batch event collision that leaves exactly
100 message/event pairs committed before a fresh worker completes the remaining five, plus the
historical-schema upgrade; successful migration and restart of Edge over the accumulated database;
and both live TypeScript privacy lifecycle journeys (5.28 seconds total).

## 2026-08-26 — Issue visibility is factored by the scope that grants it

The creation-receipt fix removed one symptom, but the underlying `issue:view` projection still
walked the same project/team/organization graph once per Issue and persisted the resulting
subject-to-Issue Cartesian product. The long-lived system tenant demonstrated the consequence:
1,678 Issues and 426 subjects occupied 494,678 projection rows, and an ordinary change could spend
more than twenty seconds rebuilding all of them.

The projection now models the policy it serves. Project visibility is resolved and stored once per
project and subject; only Issue-specific confidential membership and explicit confidential grants
are stored per Issue. Reads compose those three sets as `(project AND NOT confidential) OR grant`.
Issues owns that predicate and the bounded key resolver used by References, replacing an Edge-owned
copy of security-sensitive SQL. Reconciliation metrics now count projected memberships rather than
mislabeling confidential exclusions as grants.

The migration is safe under mixed-version workers. Every relevant database mutation resets an
explicit projection format generation, only the factored publisher can mark generation 2 ready,
and all new reads reject any older generation. A durable test simulates a legacy worker publishing
generation 0, proves reads remain fail-static, and proves a current worker repairs it. On the real
accumulated tenant the ready projection fell from 494,678 rows to 727, a 680-fold reduction; the
next complete privacy Vitest phase fell from 11.0 seconds to 5.3 seconds.

**Proof:** warning-free all-target/all-feature Storage, Issues, and Edge clippy; 508 Storage, 433
Issues, and 363 Edge library cases through the real backend harness; the durable inheritance,
expiry, confidentiality, restart, revocation, legacy-format, and rebuild-race story; the complete
PostgreSQL Edge Issue route and its 2,000-row query-plan bound; the R44 authorization saga; restarted
Edge; and both live privacy lifecycle journeys against the large tenant.

## 2026-08-25 — issue creation no longer waits for a tenant-wide list rebuild

The privacy system story exposed a product-scale coupling outside Privacy: an Issue authorization
binding became active in 66 milliseconds, but its creation receipt remained pending for another
21.4 seconds while `issue:view` rebuilt 494,678 subject-to-Issue grants. The exact-object workflow
had been made hostage to the availability of a bulk read model whose fail-static semantics serve a
different purpose.

Authorization status now follows the durable binding and then performs the authoritative strong
Identity check for that exact Issue. It can therefore return the completed, decrypted result as
soon as the authorization saga commits, while list and reference-card reads still refuse to serve a
missing or stale bulk projection. The PostgreSQL route story proves both sides of that boundary and
the brittle unit test that asserted SQL substrings has been reduced to the behavior it can honestly
test: canonical request identifiers.

**Proof:** warning-free all-target/all-feature Issues and Edge clippy; the focused Issues unit case;
the full real-PostgreSQL Edge issue-route journey; restarted Edge; and both live privacy lifecycle
journeys against the accumulated large tenant (3.5 seconds and 7.3 seconds; 11.0 seconds total).

## 2026-08-25 — privacy certificates prove their contents, not merely their shape

Privacy-request certificates carried plausible BLAKE3 strings, but durable reads trusted the
serialized holder counts and request identity without recomputing either digest. A damaged or
manually altered row could therefore look certified even though its claims no longer matched the
erasure that produced it.

Every holder receipt now binds its holder, operation, erased-record count, and key-destruction
claim. The enclosing certificate binds its canonical request identity, kind, scope, ordered unique
holder set, and each verified receipt. Completion rejects an invalid certificate before writing,
and every durable read verifies the stored proof and its relationship to the request row. The three
existing agent-data holders retain their historical digest context so already-issued certificates
remain readable. The full storage suite also caught that the new Chat scope migration had been
inserted into an earlier migration group; it now occupies its proper global position after 0134.

**Proof:** warning-free all-target/all-feature clippy for Storage, Chat, and Edge; all 508 Storage
library cases; two real-PostgreSQL privacy-request journeys, including direct JSONB tampering; and
the live authored-Chat-erasure system journey against the restarted Edge binary.

## 2026-08-24 — private Git work becomes useful Chat context without becoming public

A repository or pull-request reference in Chat previously stayed a bare canonical URI even for its
owner. Teaching the central resolver to call ordinary Git reads one reference at a time would have
made a message viewport an N+1 authorization and PostgreSQL path; resolving once as the message
author would have copied private titles to every later reader.

Git now contributes a typed owner projector alongside Issues and Knowledge. It canonicalizes and
deduplicates the requested repository and `repo:number` coordinates, asks the production
repository authorizer for the whole set once, verifies only the authorized exact repositories, and
loads every visible PR through one zipped-array PostgreSQL query. That query reuses the authoritative
encrypted-row decoder, omits missing records, and is bounded before storage. Missing repositories,
missing PRs, denial, malformed owner coordinates, and owner-query failure all remain the same
content-free tombstone. Hierarchical repository slugs use exact probes rather than inheriting the
flat browse catalogue's scan. The cohesive read path lives in its own `git_durable/reference_cards`
module instead of adding another concern to the oversized Git facade.

The black-box story is deliberately symmetric: two engineers each create a private repository and
PR, put all four references in one shared Chat message, and read that same persisted message. Each
reader receives their own repository and PR cards plus two tombstones, with the inaccessible PR
title absent from the entire response. The existing sequential CLI organization journey now also
requires the agent-authored PR title and canonical ref in ordinary rendered Chat output.

**Proof:** warning-free all-target/all-feature Edge and Git clippy; system-test and web typecheck;
the real PostgreSQL PR boundary including exact batch selection, title decryption, deduplication,
missing-record omission, and tenant isolation; the complete all-feature Edge and Git backend suites
(350 Edge and 565 Git library cases plus every durable/CDC/drill integration); rebuilt and healthy
Edge; all six live Chat collaboration journeys; all sixteen sequential CLI organization journeys;
both real-browser Chat product journeys; and the 99-row contract gate with zero falsely claimed.

## 2026-08-24 — Knowledge pages join the reference-card fabric

The first Chat card implementation was intentionally narrow, but its resolver owned Issues
directly. Adding another artifact that way would have grown a central switch and encouraged
single-object reads. Knowledge also had two separately constructed production APIs—one serving HTTP
and another serving agents—even though both represented the same durable page owner.

Reference-card resolution is now a composition of typed owner projectors. Knowledge contributes one
bounded, deduplicated page-summary query using the same owner-or-team visibility predicate as its
ordinary page reads; it never loads blocks or bodies. Visible encrypted titles become active
Knowledge cards. Private, missing, archived, erased-title, and unavailable pages all stay the same
content-free tombstone. Canonical Knowledge ids now have one owner-side validator shared by page
storage and Edge input handling, and invalid, empty, or over-cap batches are decided before a
database connection.

Production now constructs one Knowledge mutation/read service and shares it between HTTP, hosted
agents, MCP, and the card projector. A black-box story posts a team runbook and a private notebook
into the same shared conversation: a teammate gets one useful card and one nameless tombstone, while
the owner gets both real titles.

**Proof:** warning-free all-target/all-feature Edge and Knowledge clippy; system-test typecheck; the
complete all-feature Edge and Knowledge backend suites (349 Edge and 390 Knowledge library cases,
plus every CDC, drill, and durable integration); focused pre-storage batch-bound tests; rebuilt and
healthy Edge; and all five live Chat collaboration journeys against the final binary.

## 2026-08-24 — Chat references become viewer-scoped work cards

A canonical reference in Chat was durable and traversable, but readers still saw only its URI. The
browser and CLI therefore made people leave the conversation to learn even the title or state of
work they were allowed to see. More importantly, composing an unfurl without an owner-side batch
authorization seam would either create an N+1 query or turn the message response into an existence
oracle.

Chat history now resolves one deduplicated viewport of references for the current reader. Issues
owns the first production projector and resolves the whole key set through the same durable
effective `issue:view` projection as its list route. A visible Issue yields its title, state, icon,
and render hint; a missing, denied, stale, or temporarily unavailable Issue yields the same
content-free tombstone. Unsupported artifact types deliberately retain their canonical link until
their owner adapter exists. Resolution failure does not disclose which of those safe cases occurred,
and a system journey proves that the same stored message gives its author two useful cards while a
teammate sees one useful card and one tombstone with no private title anywhere in the response.

The Edge decrypts each message once, resolves all page references once, and attaches the card to the
structured node instead of inventing a parallel response index. The browser renders an accessible
title/state link or an inert neutral tombstone. The human CLI renders the same card inline while JSON
mode preserves the exact wire response. Production composition now builds one Chat read API and
reuses it for public Chat, MCP, and private agent threads instead of reconstructing the service from
an eight-argument registrar.

**Proof:** warning-free all-target/all-feature clippy for Edge, Issues, and CLI; web typecheck and
lint; system-test typecheck; the complete all-feature Edge, Issues, and CLI backend suites, including
the million-Issue cost-bound test and the durable visibility restart/revocation/rebuild-race test;
restarted Edge; the four-case live Chat lifecycle; the sixteen-stage CLI authentication and
collaboration journey; both real-browser Chat product journeys; and all twelve Chat browser-contract
cases.

## 2026-08-24 — CI residency proof lives at the enforced boundaries

CI Control Plane carried a 707-line test-support module that assembled fleet placement, log and
artifact residency, CDN behaviour, and self-hosted runner scope into mutable report structs. Its
inline tests and a duplicate external drill then mutation-tested those reports and printed green
summaries. The supposed self-hosted secret isolation check never reached a secret store: when an
in-memory admission boolean was true, the harness incremented the counter it later asserted was
zero. No production caller used the module.

That 908-line synthetic report layer is gone. Runner locality remains proved where the control plane
actually claims jobs; provider scope and attestation remain proved where the sandbox admits a
self-hosted runner; durable regional claims, logs, artifacts, execution, and accounting remain
exercised through the real CI persistence and sandbox paths. Storage's owner-side CDN and object
residency tests continue to cover the data boundary without CI relabelling their results.

**Proof:** the complete all-feature CI Control Plane suite (601 library cases, 22 binary cases, every
CDC and drill, real gVisor execution, the definition-cutover and terminal-accounting matrices, and
all durable PostgreSQL/object-store integrations); the three-case sandbox self-hosted-scope CDC;
the two-case Control Plane regional runner-claim drill; warning-free all-target/all-feature CI
Control Plane clippy; and the real 99-row contract gate.

## 2026-08-24 — Storage recovery proof belongs to the real owners

Storage exposed a 533-line test-support module that labelled three arbitrary in-process replay
sources “OLAP,” “Search,” and “Refs,” projected every source through the same generic `DerivedStore`,
hard-coded that none had a backup path, and hashed the resulting self-authored report into a green
certificate. Inline tests, a CDC suite, and a drill then repeated the same assertions. No production
caller constructed the module, and the generic projection could not detect drift in any of the real
derived stores it claimed to cover.

That 991-line synthetic composition and its duplicate suites are gone. Refs retains its owner-side
byte-identical live/cold projection proof; Search retains its live-consumer rebuild contract and
stronger ranked-result parity coverage; Storage retains its real OLAP bus rebuild and the actual
backup policy that refuses derived tiers. Removing the copied consumer tests also removed Storage's
otherwise-unused Search and Refs Service dev dependencies. This exposed a hidden test dependency:
the durable agent-trigger integration used UUID v4 only because an unrelated dev dependency happened
to enable that feature. Storage now declares and imports its own UUID requirement explicitly.

**Proof:** the complete all-feature Storage suite (504 unit cases plus every durable PostgreSQL,
object-store, backup/restore, re-erasure, OLAP, migration, RLS, and agent-workspace integration);
warning-free all-target/all-feature Storage clippy; the four-case owner-side Refs reindex suite; the
two-case owner-side Search reindex suite; and the real 99-row contract gate.

## 2026-08-23 — Chat E2E names what users can actually do

Chat's test-only E2E wedge supplied its own mutex-backed authorization service and reference
resolver, then declared an unfurl pane fresh at an invented age of zero seconds. Its privacy leg
assembled an in-memory message store, KMS, outbox, drafts, and read state before certifying its own
erase report. The duplicate external suite only called those functions and inspected their prose.

That 627-line harness is gone. Chat's product proof now rests on black-box TypeScript journeys that
create and page durable conversations, deliver mentions exactly once, keep private project rooms
non-enumerable, stream reference-only live frames only to readers, hand an Issue into a conversation
through its canonical reference, and keep named agent work out of public rooms. Focused Chat tests
still own the unfurl authorization/invalidation and erasure-cascade contracts, without calling them
end to end. The two missing production compositions are stated explicitly in the gap ledger.

The complete backend run also caught stale Postgres assertions that still queried messages under a
bare conversation id after channel lifecycle, membership, and messages had been unified on the
canonical `channel:<conversation>` ordering partition. Those tests now obtain the key from the same
constructor as production, and the cross-organization event path no longer spells the key itself.

**Proof:** the complete all-feature Chat suite (266 unit cases plus every CDC, drill, and durable
PostgreSQL/object-store integration); warning-free all-target/all-feature clippy; the 99-row contract
gate; rebuilt Edge and Workspace Gateway services; all eight live Chat collaboration, live-delivery,
and private-agent-thread assertions against the resulting system.

## 2026-08-23 — CI E2E crosses the product boundary

CI's two test-only E2E modules joined production value objects in memory and sealed a prose summary
as a `green` artifact. The supposed flagship never launched a runner or agent, opened a pull
request, applied a deployment, touched a database, or crossed a service boundary. Its second Rust
suite only called the same functions again and asserted phrases from their summaries.

That 1,198-line self-certifying layer is gone. Focused unit and durable integration tests continue
to own check emission, deployment approval, result deduplication, sandbox hardening, reindexing, and
reserve/settle invariants. Product-level CI proof now comes from TypeScript journeys through the
running stack: pushed definitions execute their exact commits in a real sandbox, successful and
failed logs remain inspectable, costs settle, repository visibility holds, history pages without
loss, and a scoped CI-failure automation waits for its owner before a hosted agent reads the run and
opens exactly one durable triage issue.

The complete suite then found two kinds of drift the aggregate harnesses had hidden. Durable log tests
still queried an obsolete aggregate spelling; `LogCoord` now owns the canonical per-job aggregate so
the writer and readers cannot diverge. That centralization also exposed a delimiter collision for
non-UUID coordinates. Canonical UUID jobs retain their deployed partition, while every other valid
coordinate now uses a bounded, length-framed, domain-separated digest. Two definition-cutover tests
also invoked the version-5 fence seed after production had advanced to a version-6 predecessor,
causing one failure and one vacuous green. They now use a current-predecessor alias guarded against
the production version constant.

**Proof:** the complete all-feature CI Control Plane suite, including real gVisor execution and every
durable PostgreSQL/object-store integration, followed after aggregate hardening by all 28 log-pipeline
unit cases and both five-case durable log suites; warning-free all-target/all-feature clippy; the
99-row contract gate; rebuilt CI Control Plane and Edge services; all eight live CI lifecycle,
history, and governed-collaboration assertions, then the five-case CI lifecycle once more against the
final binary.

## 2026-08-23 — Refs E2E follows real edges

The Refs service exposed a test-only E2E module which constructed an in-memory authorization
service, projection owner, lineage graph, reindex path, and restore/erasure simulation. Its inline
tests and a duplicate Rust suite endorsed three locally generated `green` values, but no production
caller used the module and none of the scenarios crossed the Refs service.

That 694-line harness is gone. The Refs product proof is now the black-box TypeScript journey that
creates real Chat references to Issues, waits for the durable projection, walks a twelve-source
fan-out page by page without loss or repetition, walks outbound links with an independent cursor,
and verifies that both directions of a private artifact's reference surface are indistinguishable
from absence to an unauthorized peer.

**Proof:** all remaining Refs Service tests; warning-free Refs Service clippy across all targets and
features; the 99-row contract gate; restarted-stack TypeScript reference-graph lifecycle.

## 2026-08-23 — Search coverage names the surface users have

`myelin-search` carried an 866-line test-only E2E wedge which built a private Tantivy corpus,
authorization adapter, projection fetcher, reindex source, erasure store, and holder registry. It
then declared three product-spanning journeys green from those process-local objects. Its only
callers were its inline tests and a second Rust suite; Edge, workers, browser, CLI, and MCP never
constructed it.

The synthetic wedge is gone. The product's current search proof is the TypeScript code-search
journey: exact coordinates from the current default-branch snapshot, no repository disclosure to
an unrelated principal, replacement of stale matches, branch isolation, promotion when work lands,
and removal after an ordinary Git delete and push. Cross-product issue, Knowledge, Chat, and CI
search is now an explicit product gap rather than a green library artifact.

**Proof:** all remaining Search tests; warning-free Search clippy across all targets and features;
the 99-row contract gate; restarted-stack TypeScript code-search lifecycle.

## 2026-08-23 — Knowledge journeys exercise the durable product

The Knowledge crate exposed a test-only “E2E wedge” which created its own in-memory identity
service, page store, reindex source, and lineage chain. It then hashed its own synthesized result
and called the hash a citable green artifact. A second Rust suite did nothing beyond rerunning and
endorsing that artifact; neither layer contacted a Myelin service.

The 634-line harness is gone. Knowledge's user-facing proof is now the black-box TypeScript story:
a person creates and safely retries a durable page, a reader can view but not overwrite it, stale
edits lose, saved content survives a reread, and references to delivery work appear as independently
pageable backlinks before disappearing cleanly when the document is unlinked.
The remaining in-memory and PostgreSQL CDC tests now agree with the production publisher's
canonical `page:<id>` / `database:<id>` ordering partitions instead of asserting artifact refs the
publisher rejects.

**Proof:** all remaining Knowledge tests; warning-free Knowledge clippy across all targets and
features; restarted-stack TypeScript Knowledge lifecycle.

## 2026-08-23 — Issues E2E means crossing the running system

Two public-looking, test-only Issues modules assembled private in-memory identity policies,
projection stores, relation graphs, and replay sources, then returned a bespoke `green` artifact.
Their only consumers were two more Rust test suites which repeated the same assertions. They never
crossed the Edge, PostgreSQL, authorization worker, Events relay, or Refs service, yet their names
claimed both the PR-pane and spec-to-ship user journeys.

Those synthetic orchestrators and their duplicate suites are gone. Component behavior remains
covered where it is implemented; user-facing Issues coverage now comes from the TypeScript story
that creates a project, waits for authorization, discovers and completes work, and creates and
removes a durable dependency with observable backlinks through the running stack.

**Proof:** all remaining Issues tests; warning-free Issues clippy across all targets and features;
restarted-stack TypeScript Issues lifecycle. Net removal: 1,200 lines of self-certifying test code.

## 2026-08-23 — Issues coverage no longer points at an in-memory board

`myelin-issues::BoardSync` presented a board-specific Firehose client, optimistic cache, reconnect,
snapshot, and mutation protocol as public product code. No Edge route, worker, browser, or CLI
constructed it. Four parallel integration/drill/E2E suites and an inline unit suite drove only that
process-local facade, while the real Issues application serves durable paged lists and mutations over
HTTP. The contract ledger therefore made live board collaboration look more complete than the
running product.

The facade and its self-contained tests are gone. Contract 3.5 remains covered by its real Events,
Chat, Knowledge, CI, notification, and Edge consumers; the Issues-only false pointer was removed.
The existing black-box Issues lifecycle remains the proof for what users can actually do today, and
live board updates are named as an open product seam rather than simulated in a library.

**Proof:** all 445 remaining Issues unit tests; the real 99-row contract-coverage gate; Issues
warning-free under clippy across all targets and features; rebuilt-stack TypeScript Issues lifecycle.
Net removal: roughly 1,350 lines of unwired implementation and self-testing scaffolding.

## 2026-08-23 — a new repository becomes useful without leaving Myelin

The browser could create a repository, but its first useful-hour story then crossed a hidden API
shortcut to create `README.md` and the CI definition. Blob pages explicitly deferred editing to a
future milestone. This made the polished product journey depend on knowledge and tooling that a
new organization does not yet have, and a lost file-edit response could tempt a client to repeat
an ambiguous mutation.

Repository pages now create the first file or another text file directly on a branch, and eligible
blob pages edit the current file in place. The browser validates one narrow mutation contract,
keeps a stable retry identity while a draft is unchanged, and submits a compare-and-swap commit;
branch protection remains authoritative. A concurrent commit is reported without discarding the
person's draft, while retrying after a lost response returns the original durable receipt instead
of creating another commit. Active CI rows refresh only while visible and stop polling once the
run is terminal, so the first repository-to-CI journey tells the truth without test-only polling.

**Proof:** 640/640 frontend unit tests; 132/132 browser stories; six live-stack browser stories
covering first-file creation, existing-file editing, CI completion, concurrent-edit draft
preservation, private agent work, public Chat, and automation; two TypeScript full-system Git
mutation stories; 85 Edge Git unit tests; 21 durable Edge Git integration tests; frontend
typecheck, lint, and production build; Rust formatting and warning-free Edge clippy.

## 2026-08-22 — private agent work is visible, resumable, and accountable

An agent could already participate in public Chat and automation runs, but a person had no coherent
place to keep a sensitive problem with one agent. The pieces that did exist stopped at API
boundaries: there was no named private-work surface, no durable continuation contract for a fresh
agent context, and no owner-visible record of who entered the associated workspace. That left the
product vision's human-and-agent collaboration model dependent on public rooms or hidden operator
knowledge.

Private agent threads now bind an owner, one active agent, an encrypted private conversation, and a
generation-fenced workspace with an explicit one-to-thirty-day retention period. The owner can
create, list, inspect, message, and resume the work through Edge and the CLI without configuring a
GitHub, Slack, Linear, or agent-provider credential. Agents consume the same thread through a
scoped run protocol. Workspace expiry is durable and reconciled, while SSH access uses one-shot
credentials, pins the routed host and workspace generation, and records only an accountable access
receipt—never commands, keys, routes, or file contents.

The web application now makes this contract a first-class Agents surface. A person activates a
least-privilege external agent without creating a credential, names the problem, chooses retention,
sends private messages, and returns to the same history after reload. The workspace pane shows exact
state and expiry, the MCP connection and CLI SSH commands, and paged workspace-entry history. Live
messages use authorized SSE reads with reconnect catch-up and a slow safety refresh. Chat and
private work share one timeline renderer rather than forking message semantics, and strict decoders
reject crossed aggregate identities at the browser boundary.

**Proof:** frontend typecheck and lint; 637/637 frontend unit tests; four live browser product
stories covering automations, public Chat, live Chat delivery, and private agent work; two
TypeScript full-system stories covering the owner/agent privacy boundary, fresh-context resume,
bounded workspace lifetime, CLI history, and workspace entry.

## 2026-08-20 — the durable inbox is one complete work-state surface

The notification contract promised individual read, snooze, timed resurfacing, and view-scoped
bulk read, but the production PostgreSQL store and public Edge exposed only the first operation.
The richer in-memory model therefore described a product people could not actually use, and the CLI
made even the idempotent mark-read call require a caller-invented retry key.

PostgreSQL now owns every inbox state transition. Snoozing is recipient-scoped, accepts only active
work and a future instant, removes the item from active pages, and lazily resurfaces due work in the
same tenant transaction used to read it. Bulk read changes only active rows matching the selected
view and leaves completed and parked work alone. A typed invalid-state outcome prevents completed
work from being revived. Edge exposes strict, bounded snooze and bulk-read routes with the same
notification capability boundary, while the CLI offers retry-safe `inbox read`, `inbox snooze`, and
`inbox read-all` commands with human-readable receipts.

The TypeScript journey now follows two real signals through NATS and the Notifications worker. A
person parks a mention without a retry key, sees it leave active work, clears only review requests,
sees the mention return when its time arrives, resolves both pieces of work, and is refused when
trying to revive completed work. The shared journey vocabulary performs these actions only through
the public API.

**Proof:** Notifications unit suite 348/348; Edge unit suite 333/333; CLI suite 168/168 across unit
and integration targets; strict Clippy for every Notifications, Edge, and CLI target/feature; live
PostgreSQL Notifications integration 2/2; TypeScript typecheck; focused notification lifecycle 5/5;
complete black-box system suite 30/30 files and 116/116 tests in 424.58 seconds.

## 2026-08-20 — mutation instructions mean exactly what they say

Several Git and authentication handlers decoded request bodies as untyped JSON and then selected
the fields they happened to recognize. A misspelled field could therefore become successful,
durable work with a default value: an alleged private repository was created with the ignored
visibility instruction, a string `draft` flag opened a normal pull request, and browser file edits
discarded the commit message the client already presented to the user. The login surface also
accepted unrelated fields and inherited the general one-megabyte request budget.

Git mutations and public authentication exchanges now cross small, typed request boundaries that
reject unknown fields and wrong types. Repository creation retains its documented legacy name
alias, but the alias and canonical field cannot be supplied together. Browser edits preserve an
explicit, bounded commit message in history and blame, while the old `web edit` default remains for
compatible clients. Empty Git actions accept only no body or an exact empty object. Device start,
approval, claim, and direct login share a four-kilobyte budget and exact schemas before they may
change durable state.

The black-box Git contract reads as a negative user story: ambiguous repository creation leaves no
repository, ambiguous editing leaves the repository empty, malformed pull-request intent opens no
pull request, malformed review intent grants no approval, and a hidden force flag cannot merge.
The browser-approved authentication story applies the same standard to every public exchange and
still completes one real short-lived CLI session. A long collaboration story also exposed that its
three independent human decisions legitimately cross the suite's default thirty-second budget; it
now lives in its own focused module with an explicit budget rather than weakening every test.

**Proof:** Edge unit suite 332/332; durable Git object-authorization suite 12/12; focused durable Git
integration for writes, browse, and merge bypass; TypeScript typecheck; focused Git lifecycle
12/12, mutation contract 1/1, authentication 1/1, and collaboration 3/3; complete black-box system
suite 30/30 files and 115/115 tests in 390.48 seconds.

## 2026-08-20 — CI history remains complete across repositories and pages

CI lifecycle tests found newly created runs by downloading only the first one
hundred visible rows. The durable store had lower-level coverage, but its two
pagination tests asserted SQL spelling rather than proving what a developer
sees through Edge. A sufficiently busy organization could therefore lose the
run under investigation from the test's horizon without revealing a product
or test failure.

The system-test vocabulary now pushes a minimal passing pipeline and locates
the resulting run by walking the public history to exhaustion. A new journey
creates builds in two repositories, crosses a one-row page boundary in exact
newest-first order, and proves that the complete accumulated history contains
each run exactly once. It also proves that an unrelated teammate sees neither
run and that cursors become stale when either the state filter or the caller's
visible repository set changes. The two SQL-source assertions are gone; the
behavior is covered at both the PostgreSQL boundary and the user boundary.

**Proof:** CI control-plane library suite 630/630; Clippy `-D warnings` for
every CI control-plane target/feature; live PostgreSQL surface integration
1/1 through the federation harness; TypeScript typecheck; targeted live CI
history journey 1/1; complete black-box system suite 27/27 files and 113/113
tests in 315.42 seconds.

## 2026-08-20 — a pull-request dashboard loses no work at page boundaries

Pull-request listing had extensive tests that counted SQL fragments and
matched exact query spelling, but no user journey proved that the durable
dashboard remained complete while work crossed repositories and page
boundaries. Those assertions coupled refactoring to punctuation while still
being unable to detect a missing or duplicated pull request in the product.

The system-test vocabulary now opens a proposed change through the ordinary
Git and Edge surfaces and reads the public page envelope as a typed value. A
new multi-repository journey opens four real pull requests, verifies exact
repository counts, moves forward and backward across one-row boundaries,
walks the complete accumulated dashboard without a duplicate or omission, and
proves that repository and bucket cursors cannot be replayed in a different
scope. The three superseded SQL-source assertions are gone.

The first focused run also caught a separate dishonest shed drill: it admitted
service work, never released it, then expected a lower-priority request to fit
inside the same deliberately tiny budget. The drill now models the request
lifecycle explicitly and states the real invariant—a service identity sets
the scheduling ceiling, while a header may only lower it.

**Proof:** Git crate suite 567 unit tests plus every non-feature integration
test; Clippy `-D warnings` for every Git target/feature; TypeScript typecheck;
targeted live dashboard journey 1/1; complete black-box system suite 26/26
files and 112/112 tests in 295.91 seconds.

## 2026-08-20 — a Chat mention reaches one visible teammate

Chat could deserialize and render mention nodes, and Notifications could fan
them out, but no production write path connected the two. The public message
shape only accepted a separate artifact-reference list, so it could not
express an ordered mixture of mentions and work references. The notification
read gate also checked a Chat URL directly with the wrong permission instead
of reducing it to the message or channel object that owns visibility. A
durable mention row was consequently invisible even when the recipient could
read the conversation.

Messages now accept one ordered, tagged node list while retaining the old
reference-only subset. Mention identities are loaded from the tenant-scoped
durable directory and must be active, different from the author, and able to
read the conversation; missing and inaccessible recipients have the same
response. The message, its visibility tuple, reference edges, and one
de-duplicated `signal.opened` event are committed in the same PostgreSQL
transaction. Inbox reads map Chat references back to the exact ReBAC object
and permission. The CLI exposes the result as `chat mention`, without a Slack
credential or provider key.

The black-box journey first proves that mentioning a known teammate into a
private room writes no message. In a shared room it retries one reviewer post,
reads back the structured mention once, follows the real outbox and broker
through the Notifications worker, and finds exactly one direct unread item in
the addressed teammate's inbox. Its first run exposed the broken Chat inbox
authorization reduction; the unchanged journey passed after that bug was
fixed.

**Proof:** Chat unit suite 283/283; Edge unit/integration suite green; CLI unit
suite 147/147; Chat lifecycle TypeScript system journey 3/3 against the live
federation stack; complete TypeScript system suite 25/25 files and 111/111
tests; TypeScript typecheck; Clippy `-D warnings` for every Chat, Edge, and CLI
target/feature.

## 2026-08-20 — an outbox event has one durable identity

The PostgreSQL relay silently clamped negative sequence and retry counters to
zero. A corrupt row could therefore re-enter the domain as plausible state.
Several publishing paths also decoded the serialized envelope without proving
that its event, aggregate, and subject matched the columns used to order and
acknowledge the row. A split identity could be published under one event ID
and marked complete under another.

All durable row reads now use checked counter conversions and bind the three
stored identity columns to the envelope before returning or publishing it.
Retry bounds must be positive and fit PostgreSQL, and raw enqueue paths refuse
negative sequences before SQL. Forward-only foundation migrations add the
same counter and identity invariants to PostgreSQL without changing the frozen
original outbox migration. Their bounded backfills refuse canonical sequence
collisions, repair legacy ordering identities, retain released quarantine
barriers in a resolution ledger, and canonicalize old Chat partitions to
`channel:<conversation>`.

The live upgrade journey begins with a deliberately corrupt legacy row. It
proves the decoder fails closed, the validation migration refuses to bless the
row, an explicit repair lets the migration resume, and subsequent direct SQL
corruption is rejected. It also reproduces both historical producer shapes and
proves their event, quarantine, audit, and replay states move together. On the
shared product database the migration repaired 665 split identities and 670
legacy Chat partitions without a sequence collision. The restarted publisher
then drained every affected Knowledge and Chat event to the real broker; none
remain unsent or quarantined.

**Proof:** event suite 215/215; focused relay unit tests 8/8; live PostgreSQL
outbox invariant and upgrade journey 1/1; boot migration journey 2/2; durable
outbox parity 8/8; live publisher replay with zero affected events left
unsent; Clippy `-D warnings` for every storage and event target/feature.

## 2026-08-20 — a KMS restore is one exact state transition

KMS snapshot restore performed independent root, KEK, and DEK upserts. A
failure halfway through could leave a recovery target with a root and only
some of its keys. It also left any target-only keys untouched, so “restore
this snapshot” meant merge rather than reproduce; stale key material absent
from the backup could survive recovery.

Restore now validates the complete snapshot before acquiring a transaction:
identities are unique and canonical, wrapped key shapes are exact, epochs fit
the durable representation, and every DEK names a tenant KEK at the same
epoch. It then replaces that cell's KEK/DEK membership and root in one
PostgreSQL transaction. A refused snapshot changes nothing; a valid retry is
idempotent and exact.

The durable recovery journey now creates target-only key material, refuses an
invalid snapshot while proving that material remains readable, applies the
valid snapshot, and proves both that original data recovers and that shredded
and target-only keys are absent from the database.

**Proof:** complete PostgreSQL durable-KMS integration 11/11 against the live
federation database; KMS codec tests 2/2; Clippy `-D warnings` for every
storage target/feature.

## 2026-08-20 — key epochs cannot wrap across PostgreSQL

Durable KMS key epochs are unsigned in the cryptographic domain and signed in
PostgreSQL. Individual lookup paths rejected negative rows, but boot loading
and other paths still cast them unchecked; writes also cast epochs above
`i64::MAX` to negative values. The same corrupt row could therefore be refused
by a live lookup yet accepted as an enormous epoch during restart.

Every KEK and DEK read/write path now uses the same checked epoch codec,
including boot, lookup, backup snapshots, restore, and atomic rotation. A
forward database invariant rejects negative epochs even from direct SQL or an
older writer. The durable integration journey provisions real key material,
attempts to corrupt each epoch column, observes the named database rejection,
restarts from the unchanged rows, and decrypts the original ciphertext.

**Proof:** storage unit suite 514/514 (including KMS codec and migration
catalog); complete PostgreSQL durable-KMS integration 11/11 against the live
federation database; Clippy `-D warnings` for every storage target/feature.

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

## 2026-08-23 — chat dispatch fiction replaced by a product boundary

The Chat crate exposed an in-memory "explicit dispatcher" as production API,
but no service called it. Its mock identity, wallet, effect sink, and flagship
tests could all be green while the shipped product did something unrelated.
The duplicate dispatcher, its feature-gated flagship, and four mock drills are
gone. The load-bearing agent attribution model now lives alone in
`provenance.rs`.

The replacement proof is a user journey against the live Edge: activating an
agent does not make it a public-room member, a public mention is rejected
without writing a message, and no private thread or workspace is silently
provisioned. Deliberate agent work remains the named private-thread flow.

Proof: Myelin Chat's remaining 269 unit tests and contract suite pass; clippy
`-D warnings` passes for Chat, the gateway, and Edge; TypeScript system tests
typecheck; the restarted full stack passes the four Chat lifecycle journeys.
Net removal: roughly 1,800 lines of production-shaped doubles and tests.

## 2026-08-23 — automation guards have one durable truth

The Query crate's generic `DispatchTier` was another uncalled implementation
of agent dispatch. Its in-memory balance, breaker, reconnect, replay, and loop
guard drills tested only themselves. The shipped automation path instead runs
through `GovernedTriggerConsumer` and the transactional agent-trigger store,
where self-authored events are ignored and causal-depth, privacy, firing-count,
ownership, lifecycle, and idempotency gates are applied while locking the
durable binding.

The orphan module and its four drill files are gone. The full-system automation
journey now publishes three otherwise matching events: the agent's own echo, a
human event beyond the configured depth, and a shallow human event. Only the
last reserves a firing, leaving one durable history row and one unit of budget
used.

Proof: 101 Query unit tests plus its contract suite pass; clippy `-D warnings`
passes through Query, Agent Service, and Edge; TypeScript typecheck passes; the
restarted stack passes all six automation-delegation journeys. Net removal:
roughly 1,480 lines of unused implementation and self-referential drills.

## 2026-08-23 — the trigger consumer no longer carries a shadow guard framework

Following the dispatch-tier removal exposed a second unused composite in Agent
Service. Production called only `SelfGuard`; reference gating, causal breakers,
pool accounting, and an in-memory effect ledger were public solely for two Rust
drill files. The trigger consumer now says what it does directly with
`is_agent_echo`, while causal depth and firing capacity remain transactionally
owned by the durable trigger store.

Proof: 299 Agent Service unit tests and the remaining contract suite pass;
clippy `-D warnings` passes through Agent Service and Edge; after rebuilding and
restarting Edge, all six automation-delegation system journeys pass again. Net
removal: roughly 940 lines of unused abstractions and their private simulation.

The same pass also removed Agent Service's final 160-line mention classifier.
It had no caller beyond two assertions in a mixed drill and duplicated the
already-live Chat boundary: public-room membership is explicit, private agent
work starts only through a named thread, and automations start only through a
durable trigger binding. The remaining 293 Agent Service unit tests, ten real
consumer-tool drill cases, and downstream clippy all pass.

The subsequent surge audit removed one more convincing double. `dispatch_surge`
combined an in-memory shed lane, an in-memory cost ledger, and a pretend runtime
which accumulated `Retry-After` integers. No request, event consumer, hosted
worker, or workflow called it. The production Edge already owns synchronous
HTTP/Git admission and proves the protected human lane against a live 300-request
storm; governed automation is durably queued and handed to partitioned workflow
workers. Removing the fake AgentMention front makes the remaining asynchronous
worker-admission gap visible instead of reporting it GREEN. Net removal: roughly
760 lines of unused implementation and self-testing drill.

The same audit retired `cost_gate`, a mock-brain loop that declared itself GREEN
after driving an in-memory wallet through a closure. Its only caller was the drill
that assembled those doubles. The real safety property remains covered where it
lives: Agent Service tests exercise reserve/settle semantics, and live-Postgres
host tests debit the durable organization wallet, refuse an exhausted run without
going negative, reconcile terminal reservations, and prove replay does not bill
twice. Net removal: another roughly 650 lines of production-shaped test machinery.

## 2026-08-23 — pull-request listing has a narrow durable boundary

The durable pull-request store had accumulated query generation, schema setup,
transaction orchestration, mutation replay, and projection repair in one file.
Its two pagination query families and cross-repository input validation are now
isolated in a private `list_queries` module. The store keeps ownership of binding
parameters and decoding, so the extraction changes no transaction or tenancy
boundary while giving the intricate keyset SQL a focused home for the next pass.

Proof: all 563 Git unit tests pass; Git and Edge are warning-clean under clippy;
the live-Postgres pull-request boundary proves isolation, atomicity, idempotency,
pagination, and merge recovery; after restarting Edge, all twelve TypeScript Git
lifecycle journeys pass through the public HTTP surface.

## 2026-08-23 — private agent commands run where the work persists

The old `AgentExecGate`, `SandboxToolHands`, and long-park dispatcher looked like
the untrusted-compute half of the agent product, but no service constructed any
of them. Their backends existed only inside their own tests: inline execution
returned a synthetic guest id after immediately killing the guest, asynchronous
execution parked against an in-memory signal store, and the escape attestation
was rebuilt from a synthetic console transcript. None could execute a command
in the durable workspace used by a private agent thread.

That 3,249-line false front and its self-referential drills are gone. The real
boundary is now shipped: `workspace.exec` opens the opaque storage locator bound
to the exact live agent run, revalidates it as a verified mount, and launches the
production gVisor workspace session with deny-all networking and bounded time,
memory, processes, temporary storage, and combined output. The same durable run
lease is checked while the command is active, so closing or expiring the private
thread terminates the sandbox instead of leaving unowned compute behind.

Command admission uses the encrypted durable tool-effect journal in at-most-once
mode. A completed retry returns the exact recorded exit and output; a crash after
admission but before recording the result is reported as indeterminate and is
never silently executed twice. The public receipt names the workspace generation
and bounded result, while governance audit records only the event and opaque
workspace reference.

Proof: focused Agent Service, Storage, MCP, Edge, and Agent Host unit suites;
warning-free clippy across all targets and features of those crates; live
PostgreSQL tests for thread/run locator binding and both retry policies; and the
three rebuilt-stack TypeScript private-thread stories, including exact replay,
fresh-context and SSH visibility of command changes, timeout and combined-output
enforcement, recovery after retired sandboxes, retry conflicts, and active
cancellation at workspace expiry.

## 2026-08-23 — CI cache coverage no longer means a process-local map

The trust-scoped CI cache had the same misleading shape. No pipeline definition
could request a cache, no runner restored or saved one, and the `cache_entry`
schema had no store. `CiCacheNamespace` put blobs behind a mutex-protected
`HashMap`; even its “real object store” integration constructed that map inside
one test process, so it proved neither durable lookup nor runner isolation.

The in-memory namespace, its duplicated poison drills, and a synthetic unified
sandbox CDC are gone. The historical `cache_entry` migration stays immutable,
but it is now clearly unused schema awaiting a real design. The contract ledger
also stops pointing at deleted mock tests and marks `ToolHands::exec` deferred to
the production workspace-exec landing rather than manufacturing coverage.

Proof: 624 Storage and 511 CI Control Plane library tests pass; all Storage, CI
Control Plane, and Edge targets are warning-clean under clippy; the contract
coverage gate reconciles all 99 rows with no missing paths; after rebuilding the
live services, all five TypeScript CI delivery journeys pass, including real
sandbox output, failure settlement, and repository visibility. Net removal:
roughly 1,430 lines of non-durable implementation and self-testing scaffolding.

## 2026-08-24 — CI results remain useful when they enter a conversation

A CI run reference in Chat used to remain an opaque UUID even for its owner. It
now resolves through an exact, bounded PostgreSQL summary read and one exact Git
visibility decision. The card names the repository and current run state without
loading jobs, steps, logs, or artifacts. Missing, malformed, inaccessible, and
temporarily unavailable runs all collapse to the same content-free tombstone.
The shared Git visibility seam validates canonical slugs, authorizes the requested
set once, and rejects directory-shaped impostors by consulting the durable store.

The TypeScript journey creates and executes two private repositories, puts both
opaque run references in one shared message, and reads it as each engineer. Each
reader sees only their own useful card and cannot find the other's repository name
anywhere in the response. The Chromium product spine now carries its real sandbox
result into Chat, follows the projected card, and lands back on the durable run.

Proof: all 351 Edge and 602 CI Control Plane library tests; the real PostgreSQL
run-surface integration with RLS and exact duplicate/missing ids; warning-free
clippy across all targets and features of both crates; 640 browser units; six
live TypeScript CI journeys; and the real Chromium product journey after an Edge
restart.

## 2026-08-24 — a private conversation stays resumable by reference

Canonical Chat conversation references now resolve to the topic for a current
member and to a content-free tombstone for everyone else. Exact resolution uses
the same durable SQL predicate as conversation listing: public rooms inherit a
live project view, private rooms require unexpired direct membership, archived
or unstamped rooms remain absent, and all coordinates are tenant/region scoped.
The shared query shape removed the previous duplicated list predicate while
adding a bounded, canonical, deduplicated exact read.

The TypeScript story gives two engineers separate private topics, carries both
references into one shared room, and reads the same message from both identities.
Each engineer sees only the topic they can resume; neither response contains the
other topic's name. In Chromium, an engineer links a second encrypted durable
topic from the first and follows the projected card into that exact conversation.
This also makes the canonical conversation reference returned by a private agent
thread a useful fresh-context handoff without broadening its membership.

Proof: all 268 Chat and 351 Edge library tests through the database-backed
harness; the real PostgreSQL membership-expiry integration; warning-free clippy
across every Chat and Edge target and feature; 640 browser units; all seven live
Chat lifecycle stories; and both real Chromium Chat journeys after rebuilding
Edge.

## 2026-08-24 — named private agent work survives a fresh-context handoff

The canonical reference returned by a private agent thread now resolves to its
name and workspace lifecycle only for its owner. Everyone else receives the same
content-free tombstone used for missing and unavailable work. Resolution is one
bounded, deduplicated PostgreSQL query over the exact requested UUIDs, scoped by
tenant, region, and owner. Deleted rows remain available through the lifecycle
receipt API for audit, but are excluded from reference projection because their
workspaces can no longer be resumed.

The TypeScript story gives two engineers their own external agent and named
three-day workspace, carries both thread references into one shared room, and
reads the same message as each person. Each sees only the work they own; neither
response contains the other thread's name or workspace id. The Chromium product
journey now creates private work through the UI, reloads its durable message,
links the thread from Chat, and follows the projected card back to the exact
workspace. This makes a canonical thread reference a genuine fresh-context
handoff instead of an opaque identifier.

Proof: the complete Storage and Edge library suites; the real PostgreSQL private
thread lifecycle including exact owner reads and post-cleanup exclusion;
warning-free clippy across every Storage and Edge target and feature; 640 browser
units; all eight live Chat lifecycle stories; and the real Chromium private-agent
workspace journey after rebuilding Edge.

## 2026-08-25 — an exact Git revision becomes useful conversation context

A canonical Git commit reference now resolves to its bounded summary for a
viewer who can read the repository and to a content-free tombstone for everyone
else. Repository visibility is decided once over the union of repository,
pull-request, and commit coordinates. Visible commit metadata is then read in one
repository-open batch per repository rather than reopening the object store for
every card. Only a full lowercase object id is accepted; missing or malformed
objects disclose nothing. Hostile or oversized commit summaries fall back to a
safe repository-plus-short-id title or a UTF-8-safe 512-byte prefix.

The TypeScript story gives two engineers private repositories, pull requests,
and uniquely named commits, then carries all six references in one shared
message. Each engineer sees useful cards for their own three objects and no name
from the other repository. In Chromium, an engineer creates a real commit,
links its canonical reference from Chat, follows the card to the exact diff, and
returns to the durable conversation.

Proof: the complete Git and Edge library suites; focused batch, canonical-shape,
and hostile-summary tests; warning-free clippy across every Git and Edge target
and feature; 641 browser units; all eight live Chat lifecycle stories; and both
real Chromium Chat journeys after rebuilding Edge.

## 2026-08-25 — branches and tags become private, navigable Chat context

Canonical Git ref event coordinates now resolve to useful branch or tag cards
for repository readers and content-free tombstones for everyone else. The Git
projector accepts only canonically encoded repository/ref components and only
the browsable `refs/heads/*` and `refs/tags/*` namespaces. It authorizes the
union of Git coordinates once, deduplicates exact ref names, and opens each
visible repository once for the bounded ref batch. Missing refs stay absent;
malformed and symbolic data fail closed.

The TypeScript privacy story now gives each engineer a distinct branch beside
their repository, pull request, and commit. Each person sees a named, navigable
card for all four of their own objects while the other repository's four names
remain undisclosed. Browser parsing independently rejects abbreviated,
non-canonical, or non-browsable coordinates and routes valid branch/tag cards
to the exact tree.

Proof: the complete Git and Edge library suites; focused exact-ref,
canonical-coordinate, deduplication, and UTF-8-boundary tests; warning-free
clippy across every Git and Edge target and feature; 642 browser units; and the
eight rebuilt live Chat lifecycle stories.

## 2026-08-25 — one canonical Git file coordinate reaches Chat and the browser

Git file references no longer have two producer spellings. Code search
projection and line-range minting now share `GitBlobEventKey`, which owns the
repository, full branch/tag-or-exact-commit selector, safe path, canonical
subject-component encoding, parsing, and bounded shape. The weaker
`repo:main:src%2Ffile.rs` helper and its slash-only codec are gone; producers now
emit the same coordinate the durable owner and browser consume.

For a repository reader, Chat verifies an exact bounded set of file locations
through one repository open and projects a file card without reading file
contents. Missing paths, directories, hidden repositories, and malformed
coordinates become content-free tombstones. Canonical line-range subanchors are
preserved on the card and in the exact browser URL. The two-owner TypeScript
story now compares each engineer's repository, pull request, commit, branch,
and file without revealing the other engineer's card titles. Chromium follows
a real `README.md#L1-L1` card into the repository file viewer and back to the
durable conversation.

Proof: the complete Git, Edge, and Refs-service library suites; the Git 5.7
provider/consumer and anchor contracts; focused canonical-coordinate,
exact-file-batch, and line-range tests; warning-free clippy across all three
crates and every target/feature; 643 browser units; all eight rebuilt live Chat
lifecycle stories; and both real Chromium Chat journeys.

## 2026-08-25 — an assigned review remains useful without repository access

A requested reviewer can now open the exact pull request and comments entrusted
to them without receiving repository enumeration or clone access. The same
durable review relationship governs the PR HTTP boundary and Chat projection;
submitting a decision completes the active request without erasing the
reviewer's historical access. PR summaries, merge gates, and both PostgreSQL
needs-review queries now derive each reviewer's state from their latest durable
decision, so an old request, approval, or block cannot survive a newer one.

Chat comment cards are resolved from one exact, viewer-scoped comment batch.
Pending review comments remain author-only, removed bodies never resurface, and
missing or denied coordinates become content-free tombstones. PR-level comments
route to the overview; line-anchored comments carry their live, moved, or
outdated placement and route to the exact comment on the diff. The TypeScript
story assigns a reviewer, gives them both kinds of comment, proves the repository
stays hidden, submits an approval, and proves the entrusted context remains
readable. Chromium follows both cards to their real rendered comments.

Proof: the complete Git and Edge library suites; the live PostgreSQL list
transition from requested to approved; warning-free Git/Edge clippy; 643 browser
units; TypeScript typechecks and lint; all nine rebuilt live Chat lifecycle
stories; and both real Chromium Chat journeys.

## 2026-08-25 — one exact private message is a resumable handoff

A Chat message now has one durable tenant-and-region-scoped identity outside its
parent timeline. The forward-only `chat_0009` migration enforces that identity;
metadata-only batch lookup powers cards, while the exact content endpoint reads
one row and then rechecks the parent conversation's live membership before it
decrypts anything. Both the root reference and its matching message subanchor
resolve to a topic-named card for a viewer who can still enter the room. Missing,
malformed, mismatched, and denied coordinates remain content-free tombstones.

The browser opens `/chat?message=…` through that exact endpoint rather than
assuming the message sits in the latest page. It renders the focused message at
its stable DOM anchor and offers a clear return to current conversation traffic.
This work also closed an adjacent erasure bug: deleted and tombstoned rows no
longer ask the KMS to decrypt deliberately destroyed bytes and take the whole
timeline down; they render as an explicit removed-message state with no body or
nodes.

The TypeScript story gives two engineers separate private messages, carries both
references into one shared handoff, and reads it as each identity. Each engineer
sees and can open only their own exact words; neither the private topic nor body
crosses the reciprocal denial. Chromium creates a real referenced message,
follows its projected card to the exact focused view, verifies its anchor and
body, and returns to the latest conversation.

Proof: all 269 Chat and 359 Edge library tests; the real PostgreSQL exact-lookup,
deduplication, and cross-tenant RLS integration; an immutable-migration restart
through `chat_0009`; warning-free Chat/Edge clippy; 643 browser units plus
TypeScript and ESLint; all ten live Chat lifecycle stories; and both real
Chromium Chat journeys.

## 2026-08-25 — focused replies stay beside the room

A reply no longer has to become another room or disappear into an undifferentiated
timeline. A top-level Chat message is now a stable thread root; every reply points
directly to that root, and a root from another conversation or a reply masquerading
as a root is rejected. Room pages contain only top-level messages and their exact
reply counts. Thread pages return the root separately from their pageable replies,
so a long discussion never pages away the sentence that gave it meaning.

The read and write boundaries recheck the live parent-conversation permission
before decrypting or appending. Exact retries return the original reply, while the
same retry key carrying different words is a conflict rather than silent data loss.
Canonical thread references resolve to useful topic-named cards for current members
and content-free tombstones for everyone else. The browser can enter a thread from
its room, reply in place, follow the reference back to the exact anchor, and return
to current room traffic. The CLI offers the same `chat reply` and `chat thread`
workflow without asking a person to reconstruct HTTP routes.

The implementation also leaves a calmer shape behind: indexed root/reply reads are
owned by a focused PostgreSQL module, thread routing is separate from conversation
orchestration, and encrypted message rendering has one narrow module. The TypeScript
story reads as a product decision: two replies deepen one decision while an ordinary
message remains visible in the room, retries are honest, paging keeps the root, and
a private thread discloses neither topic nor body.

Proof: all 269 Chat, 360 Edge, and 154 CLI library tests; the real PostgreSQL
root/reply range integration; warning-free clippy across all targets and features of
all three crates; TypeScript and ESLint; 644 browser units; all twelve rebuilt live
Chat lifecycle and CLI stories; and both real Chromium Chat journeys after an Edge
restart through the immutable `chat_0010` migration.

## 2026-08-25 — a reply becomes one durable nudge

The author of a thread root now receives one unread `replied` inbox item when a
teammate answers, linked to the canonical thread rather than to copied message
content. Replying to one's own root is quiet, and an exact transport retry remains
the same message and the same notification. The root author's delivery identity is
recorded beside the root in a tenant-and-region-scoped companion table in the same
transaction as the message. A partial unique index makes the single-author role a
database invariant while leaving room for many future thread participants.

The durable append boundary now owns the thread invariant instead of trusting Edge:
under a row lock it requires an active top-level root from the same conversation.
The reply, `chat.thread.replied` domain event, addressed `signal.opened`, visibility
tuple, reference edges, and mention signal then co-commit. A nested reply, a root
from another room, or a removed root cannot become a false thread. The notification
contains routing identity and the thread coordinate, never the message body, and
inbox reads still reauthorize that coordinate against live room membership.

Roots written before `chat_0011` remain readable and replyable but cannot acquire an
author notification retroactively: Chat deliberately stores only an event
pseudonym in the old row, so there is no safe real identity to reconstruct. A
successful reply now uses the participant model to follow later activity; an
explicit mute or unfollow control remains a separate user-facing seam.

Proof: all 271 Chat and 360 Edge library tests; the real PostgreSQL co-commit,
idempotency, false-root, and tombstone integration; warning-free Chat/Edge clippy;
TypeScript typechecking; and all eleven live Chat collaboration stories after an
Edge, notifications, and outbox restart through the immutable `chat_0011`
migration; the single-author index follows in forward-only `chat_0012`.

## 2026-08-25 — participating keeps a thread within reach

A person who replies now follows that exact thread automatically. When someone
else answers later, prior participants receive one lower-priority
`thread_watched` inbox item while the root author keeps the more direct `replied`
item. The current replier never notifies themself, the root author is not sent a
second lower-priority copy, and neither signal contains message text. Existing
notification deduplication coalesces later activity by recipient, rule, and
canonical thread.

Participation is recorded in the same transaction as the reply and all derived
events. Replies take an update lock on their root, giving concurrent replies a
real order: the later transaction sees the earlier participant before it derives
its audience. Exact retries add neither another participant nor another event.
The producer orders and caps watched recipients at Notifications' 64-recipient
hot-subject bound, so a large thread cannot turn one reply into an unbounded
payload or database read. Ordinary channel watchers remain read-fanout and cause
no per-member inbox writes.

The TypeScript story now states the product behavior directly: a reviewer joins a
decision thread, receives no notification for their own reply, and is brought
back when the founder answers. At this checkpoint participation was the only
thread-follow action; the next seam adds an explicit quiet control without
discarding that durable participation history.

Proof: all 273 Chat library tests; the real PostgreSQL reply, participant,
notification, idempotency, false-root, and tombstone integration; warning-free
Chat/Edge clippy across all targets and features; TypeScript typechecking; the
focused red-to-green lifecycle story; and all eleven live Chat collaboration
stories together.

## 2026-08-25 — a followed thread can become quiet without becoming lost

Following is now an explicit, durable property of a Chat participant rather than
an inference a client has to reconstruct. Replying still follows a thread, but a
person can mute it idempotently, see that state on the thread, and follow it again
when the decision matters. Muting changes only notification delivery: it does not
delete the participant, change their role, or erase how they joined the work. A
later reply is an intentional act of participation and follows the thread again.

Reply, follow, and mute all lock the same active root before changing participant
state, so concurrent actions have one database order and the last user action
wins. The forward-only `chat_0013` migration adds the delivery flag without
changing the checksum of the participant migration already deployed. Both HTTP
mutations recheck live room visibility and return the same content-free 404 as a
private thread read or reply for an outsider.

The end-to-end story exposed a neighboring attention bug: notification collapse
incremented its counter but left a read or completed card hidden. Fresh activity
now reopens ordinary finished work, preserves an explicit snooze, and advances
provenance and recency monotonically so a late event cannot move the card backward.
The story reads as a person working: join a decision, finish its current nudge,
quiet it, observe uninterrupted work, return deliberately, and receive the next
reply on the same durable card.

Proof: all 273 Chat, 349 Notifications, and 360 Edge library tests; strict
all-target, all-feature Clippy for all three crates; real PostgreSQL Chat
co-commit and Notifications collapse/reopen integrations; TypeScript typechecking;
and focused live privacy and mute/follow lifecycle stories after immutable
migration and service restarts.

## 2026-08-25 — the CI completion path has names again

The durable CI completion path no longer hides dispatch, legacy test driving,
pricing, receipt derivation, and retry accrual inside one roughly 3,000-line
module. Each concern now has a narrow module and the original facade preserves
the existing API. The production settlement/reporter core has fallen to 1,857
lines without moving tests into production or weakening any claim fence. A
reader can inspect how a dispatch is made, how a replay receipt binds a claim,
or how retry usage is accumulated without first understanding the other two.

The split also found a real refusal-path bug. Tier-P settlement validation
recomputed the pinned CPU and memory prices with unchecked `u64`
multiplication. A malformed pricing implementation and large-but-durable usage
could therefore panic in a debug build or wrap in an optimized one. Expected
prices are now checked arithmetic; unpriceable usage returns the existing
`InvalidOutput` refusal. A regression uses `i64::MAX` CPU seconds to pin that
fail-closed behavior.

Proof: all 603 CI control-plane library tests and warning-free all-target,
all-feature Clippy; both real PostgreSQL pipeline culmination stories; all 27
atomic terminal-accounting scenarios; and the focused eight-story live retry
and race subset. The latter covers exact requeue, router handoff, budget
exhaustion, unresolved phases, retry-versus-phase races, cancellation usage,
and retry-versus-supersession ordering.

## 2026-08-25 — rebuilding issue visibility no longer stops issue creation

The complete Chat lifecycle exposed a durable latency defect outside Chat: an
issue visibility rebuild held the tenant projection revision lock while it
walked every relevant ReBAC path twice. Issue creation invalidates that same
revision row, so an ordinary proposal waited behind the full computation. In the
accumulated system-test tenant, one proposal spent 11.7 seconds inside its HTTP
request before it could even enter the documented asynchronous authorization
hold.

The rebuild now takes a revision snapshot and computes one bounded recursive
walk into transaction-local staging without the publication lock. Validation and
effective membership share that staged walk instead of repeating it. Only the
short replacement-and-publication phase locks the revision. If a grant, revoke,
or issue changes during computation, the worker returns an explicit
`Superseded` outcome and discards the snapshot; the stale revision is never
published, and reads remain fail-static until a fresh rebuild wins.

The PostgreSQL race story pauses a fully staged snapshot for two seconds and
requires a concurrent revoke to commit within one. It then proves the stale
snapshot was refused and that a fresh rebuild excludes the revoked issue. On the
same accumulated tenant, live issue proposals returned to roughly 100 ms without
relaxing the existing authorization or story budgets.

Proof: all 433 Issues library tests and focused Edge reconciliation tests;
warning-free all-target, all-feature Clippy for Issues and Edge; the real
PostgreSQL visibility/revocation race, issue saga, and Edge route integrations;
and all twelve live Chat collaboration stories together after an Edge restart.

## 2026-08-25 — an erasure request is a durable user object

Agent-data erasure is no longer only an irreversible button backed by one HTTP
response. A human can submit one named, idempotent privacy request, recover its
status by ID, and retrieve a content-addressed certificate. The request is
tenant- and owner-scoped, survives a worker loss, reclaims an expired lease with
an epoch fence, and publishes its certificate only after every registered
holder has supplied a complete proof. Today the deliberately honest holder set
is the production agent-data path: traces, model replay, and tool effects all
share one scoped agent-data key and all three report their own deletion counts.

Review found a crash seam in the first composition: a process could destroy the
key and delete the rows, then die before storing the request certificate. A
retry remained safe but could only report zero deletions. The subject-erasure
marker now records the three counts in the same database transition that
deletes the rows and marks completion. A fresh holder instance recovers that
proof, the database trigger makes a completed proof immutable, and the privacy
request can finish after the original worker disappears. This replaced the
agent-trace module's broad test that inspected migration source strings with a
real PostgreSQL attempt to mutate the completed receipt and an assertion that
the database refuses it.

Proof: TypeScript typechecking; all focused Storage and Edge privacy and
authorization tests; warning-free all-target, all-feature Clippy for Storage
and Edge; real PostgreSQL holder-restart/receipt-immutability and request
lease-loss stories; and the live black-box privacy lifecycle. The live story
creates recoverable agent work, submits and replays one request, reads its
status and three-holder certificate, then proves both the old result and all
future processing stay unavailable. It completed in 13.68 seconds.

## 2026-08-25 — restore replay now restores the refusal, not only the key

The old restore drill replayed a post-PIT erasure through a generic KMS pass
whose other holders were no-ops. It proved the resurrected key was destroyed at
that instant, but it did not recreate the durable agent-data subject marker. A
later agent run could therefore treat the restored subject as active and mint a
replacement key. The drill was green while the user-level promise was still
broken.

The production restore primitive now selects post-restore subjects from the
preserved live ledger and sends each one through `DurableAgentTraceStore`, the
same holder used by the privacy request. That path deletes restored traces,
model replay, and tool effects; destroys and verifies the key; and commits the
absorbing marker. A fresh invocation safely replays the same cutoff and returns
the original durable counts without reopening processing.

Edge now exposes this as the maintenance-only `privacy-reerase` command. It
requires a canonical restore timestamp, exact cell confirmation, explicit
confirmation that serving processes are stopped, and a separately preserved
ledger database. Database identity comparison ignores credentials, so changing
users cannot disguise the restored target as its own supposed live ledger. The
success receipt contains aggregate counts only. The complete operator sequence,
retry rules, and failure interpretation live in
`docs/runbooks/post-restore-agent-data-reerase.md`.

Proof: three focused command/safety tests, warning-free all-target/all-feature
Edge and Storage Clippy, and the real 35.69-second `pg_dump`/`pg_restore` story.
The restored database must reject a brand-new trace after replay and a fresh
operator pass must converge as already erased.

## 2026-08-25 — narrow agent-data erasure has its own cryptographic boundary

The holder audit found a cross-product erasure hazard before adding another
ceremonial holder: the key model offered only one generic `(tenant, subject)`
DEK. The public operation is explicitly scoped to `agent_data`, so its safety
must not depend on every other product transforming the person's identifier
differently before selecting a key. Chat currently uses an author pseudonym,
for example, but that incidental separation is not a cryptographic domain
boundary. Any holder using the same raw subject coordinate would have its key
destroyed by the narrow operation.

The KMS key model now has a typed scoped-subject class. All three durable
agent-data writers use the `AgentData` scope and readers retain compatibility
with legacy unscoped ciphertext. Erasure deletes both legacy and current rows,
but destroys only the scoped key; it never destroys the older cross-product
key. The post-restore path replays the same boundary. This preserves existing
data while giving every new agent record an independent cryptographic erasure
lever. Two journal tests that merely searched migration source for expected SQL
phrases were removed; the surviving tests exercise typed key behavior and the
actual PostgreSQL constraints.

The post-restore ledger now records a typed product scope as part of its
primary key. The agent-data operator selects only `agent_data` rows; a future
Chat erasure for the same person cannot accidentally be replayed through the
agent holder. Its legacy all-holder adapter still deduplicates subjects across
scopes for the older full-fanout restore machinery. A real PostgreSQL story
records agent-data and Chat work side by side and proves both selection modes.

The black-box privacy story now writes and reads a private Chat message as the
same person before requesting agent-data erasure, verifies that the request and
certificate complete, reads the original Chat message again, and posts a new
follow-up. This states the end-user scope contract directly. The regression
with teeth is in the real dump/restore drill: it provisions an unrelated key at
the exact same raw subject coordinate and requires that key to remain resolvable
after both live erasure and restore replay; the old implementation destroys it.

Proof: four KMS grammar tests; the five-case real PostgreSQL agent-trace suite;
real model-replay and tool-effect journal tests; warning-free all-target,
all-feature Storage Clippy; TypeScript typechecking; the 34.82-second real
`pg_dump`/`pg_restore` drill; the real scope-aware ledger story; and the
15.20-second live privacy system story.

## 2026-08-25 — Chat has an independent key boundary

The next holder audit exposed the same coupling on Chat's write path. Message
bodies still selected the legacy generic subject key, so a future durable Chat
erase could not safely become one member of a larger privacy request: destroying
its key could also erase an unrelated holder that happened to use the same
subject coordinate.

New Chat bodies now select the typed `Chat` subject-key scope. Decryption accepts
both that scope and the legacy unscoped class, but explicitly rejects another
product's scoped key. The erasure cascade uses the same typed selector. This is
only a safe cryptographic seam—not a claim that the existing in-memory cascade is
a production holder.

Proof: the real PostgreSQL subject-key and existing cascade tests; focused Chat
contracts; warning-free all-target, all-feature Chat, Edge, and Storage Clippy;
and 13 live Chat/privacy system journeys in 129.31 seconds.

## 2026-08-25 — Chat message erasure reaches its authoritative store

The PostgreSQL-labelled Chat cascade test was not an integration test. It put
ciphertext in a one-off table, then invoked the cascade against a different
`MemHotTier`; no production message row was ever mutated. That test is gone.

`PgMessageStore` now owns an async authored-message erasure operation. In one
tenant-scoped PostgreSQL transaction it empties both encrypted body columns,
moves every live message by the pseudonymous author to `tombstoned`, and
co-commits one `chat.message.erased` envelope per message. Its API returns only
the count it can prove. A retry sees no live authored messages and emits no
duplicate consequence.

The replacement story writes two encrypted messages for Ada across two rooms
and a neighbouring message for Bob using the production schema. It proves that
only Ada's rows become empty tombstones, Bob's body still decrypts, exactly two
durable erasure events exist, and retry is quiet. A second story deliberately
gives two different tombstones the same event identity; the outbox rejects it
and PostgreSQL restores both message bodies. This closes the storage/event
atomicity seam, but the key, restore ledger, read state, and public DSR request
still need one durable orchestration boundary.

Proof: the two-case real PostgreSQL Chat erasure story (0.53 seconds) and
warning-free all-target, all-feature Chat Clippy.

## 2026-08-25 — Chat message erasure is resumable across crashes

The message transaction alone was atomic but its retry receipt was not durable:
after response loss, a second call could only observe zero remaining live rows.
Worse, a message append could race the interval between selecting authored rows
and completing the wider key-erasure operation.

Every Chat message erasure now begins with a tenant-scoped operation marker.
The marker is keyed by the caller's stable operation identity, binds that
identity to exactly one pseudonymous author, and permits only one transition
from pending to a completed pair of message/event counts. Retrying either phase
returns the original receipt. A failed outbox co-commit leaves the operation
pending and both message bodies intact.

Production appends and erasure share one framed author lifecycle fence. Appends
hold its shared form through their transaction; erasure holds its exclusive
form while preparing and completing. Once a pending marker exists, a new
message by that author is refused. A writer that began first must finish before
the marker can commit, so the later erase necessarily sees its row. The real
PostgreSQL story exercises the write refusal, durable receipt replay, and failed
transaction resumption. The production migration was applied by restarting
Edge, then the live delivery story proved normal Chat posting still works.

The old unit test that asserted Chat migration correctness by searching DDL
strings was removed. The schema and its RLS behavior are exercised by the real
database story instead.

The same story now covers the upgrade boundary explicitly. A message written
before scoped Chat keys used the generic cross-product subject key. Destroying
that key for a Chat-only request could erase another product, while preserving
it would leave the old message recoverable from backups. The production erase
transaction therefore inspects and locks every live body envelope before
mutation and refuses a legacy or foreign key class. It leaves the operation
pending and the body intact for an explicit re-key migration; it never emits a
successful but false crypto-shred receipt.

Proof: the three-case PostgreSQL story (0.72 seconds), warning-free strict Chat
Clippy, and two live Chat delivery journeys (633 ms of test time).

## 2026-08-25 — A privacy request can truthfully erase authored Chat history

The message-store mutation is no longer callable as if its database receipt
alone proved erasure. A private verification capability now binds storage
completion to the durable orchestrator that inventories every live envelope,
records the post-PIT obligation, destroys and rechecks the independent Chat
key, and only then commits tombstones and events. A failed event transaction is
resumable after the irreversible key step, while a completed replay returns its
original counts without touching key material created for later messages.

The privacy request model now admits a separate `chat_messages` scope. Its
worker dispatches by the request's durable scope, obtains one holder receipt
only from the complete Chat proof, and feeds the common leased certificate
path. The browser-facing story writes private thoughts, receives an exact
Chat-only certificate, sees content-free tombstones, writes again under fresh
key material, proves that replaying the old request preserves the new message,
and then erases it with a new request. Agent-data state remains unchanged.

Proof: four real PostgreSQL Chat erasure cases, two real PostgreSQL privacy
request scope cases, 363 Edge tests, warning-free strict Storage/Chat/Edge
Clippy, TypeScript typechecking, and two live privacy journeys in 15.96 seconds
(691 ms for the Chat journey).

## 2026-08-25 — Restore recovery consumes the Chat erasure ledger

The maintenance-only restore command now queries `agent_data` and
`chat_messages` independently and runs each record through its production
holder. Chat replay uses a stable, content-free operation identity, a
pseudonymized service actor, the same scoped-key verification and destruction,
and the same message/event co-commit as the public request. Its bounded report
separates newly completed work from response-loss retries and exposes no subject
identifiers.

The PostgreSQL restore story gives the preserved ledger one erasure after the
restore point and one before it. Only the newer subject loses its restored key
and body; the neighbor still decrypts. Replaying the operator returns the
durable original count without another event. The operator runbook now names
both scopes and remains explicit that Issues and Git are outside its proof.

Proof: the four-case Chat PostgreSQL suite in 0.75 seconds, four restore-command
contract tests, and warning-free strict Chat/Edge Clippy.

## 2026-08-26 — delete the HYOK proof that could not observe HYOK data

The Search quality audit found a green security drill whose central observation
was hard-coded. `HyokCrossStoreInputs::hyok_class_present_in` returned `false`
for index segments, vectors, caches, and backups without reading any of them.
The tests populated only a platform-managed control class, then reported that
all four stores had been walked and that no HYOK plaintext existed. They could
not turn red if a HYOK document were indexed.

The false SRCH-D10 artifact, verdict, gate, five unit tests, and external drill
have been removed. Three more downstream facades were equally unwired:
Search's `hyok_skips_index`, its erasure holder's `erase_class`, and Chat's
`admit_message_indexing` were called only by tests that interpreted a returned
boolean as proof that no data reached an index. Those APIs and their three
tests are gone too. The remaining file and drill are named for the behavior
they actually exercise: backup-scale erasure. That path still seals recoverable
ciphertext, purges live documents and vectors through the holder, destroys the
key, and proves the retained backup no longer opens. The public Search and Chat
exports no longer offer callers an assurance object that the system cannot
calculate.

This does not declare HYOK safe by absence. Tracing the origin model after the
downstream deletion showed that it had no production caller anywhere. Its
`Byok` implementation retained a customer-key path but wrapped with Myelin's
ordinary platform KEK; no customer-key adapter was called. The isolated
`KeyOrigin` model, its contract test, and its now-private arbitrary-envelope KMS
helpers have therefore been removed too. Contract 11.3 now distinguishes the
shipped durable platform KMS from open BYOK/HYOK work. A future SRCH-D10 must
begin with a configured customer-key origin and drive the same consumer and
stores used by live search.

Proof: 333 Search library tests, 261 Chat library tests, 417 Storage library
tests, the two-case renamed backup-erasure drill, the two real Chat ACL-search
stories, warning-free all-target/all-feature Search, Chat, and Storage Clippy,
and a workspace-wide all-target compile. The feature-enabled object-store story
reached the live adapter but could not run because that dependency was
unavailable, and is deliberately not counted as green.

## 2026-08-26 — a person can erase authored Issue titles without erasing shared work

The privacy request surface now admits an exact `issue_titles` scope. It keeps
the shared Issue coordinate, project, state, and authorization binding alive,
but replaces every title encrypted for the requesting subject with an explicit
erased placeholder and removes its ciphertext, nonce, key reference, title
subject, and direct creator identifier. A colleague's Issues and every other
privacy scope remain untouched. The certificate is issued only after the
Issues-scoped subject key is gone and one content-free `issue.updated`
consequence has co-committed per title.

The implementation closes the create/erase race instead of layering erasure on
top of it. Title encryption now happens inside the create transaction after a
shared subject-lifecycle lock and after idempotency replay has resolved. An
erasure owns the exclusive form of that lock, persists its operation before
key destruction, verifies every live title's exact Issues key class in bounded
batches, and refuses legacy generic or foreign key rows before touching the
key. `title_subject` is distinct from visible creator attribution, so work an
agent authors on a person's behalf follows that person's title-key lifecycle.
A completed operation can be replayed without erasing titles written later;
new work receives fresh key material and needs a new request.

Issue-title erasures now have their own post-PIT ledger scope. The maintenance
operator selects it independently, uses stable content-free restore operation
identities, runs the same validation/key/tombstone path against the restored
cell, and includes bounded Issue counts in its aggregate report. This is wired
production recovery behavior. A real `pg_dump`/`pg_restore` drill proves an old
backup resurrects the exact decryptable title, then the live ledger selects
only the Issue-title obligation, destroys the resurrected key, tombstones the
title, preserves a colleague's work, and leaves later work intact on replay.

Proof: 418 Issues and 417 Storage library tests, warning-free strict
Storage/Issues/Edge all-target Clippy, TypeScript typechecking, the existing
real PostgreSQL Issues authorization saga in 1.41 seconds, the new real
PostgreSQL title lifecycle from a freshly rebuilt migration in 0.77 seconds,
four restore-command contract
tests, the real Issue-title backup/restore drill in 35.94 seconds, and all three
black-box privacy journeys in 10.92 seconds. A broader
initial backend run compiled the workspace and then stopped on three Chat HITL
drill contexts that lacked the now-required durable timer wheel. Those fixtures
now share one story harness that supplies and asserts the exact durable timeout;
all three are green, and the resumed broad run progressed to the self-CI gap
below.

## 2026-08-26 — self-hosted CI restores its full quality gates

The checked-in Myelin pipeline again runs four first-class jobs: build, complete
workspace tests, warning-free workspace Clippy, and the `myelin-lints`
architecture gate. A broad backend run caught that the file had been reduced to
three weaker jobs even though its self-hosting contract still described four.

The underlying gap was a split authority. The control plane already admitted
the full structured recipes, while the gVisor launch boundary carried an older
four-command copy of the allowlist. The earlier operational workaround weakened
the pipeline to what that copy recognized. The sandbox now owns both structured
Cargo recipe recognition and deterministic offline argv lowering; the control
plane consumes those functions. At launch, the sandbox removes exactly one
platform-owned vendor configuration frame, validates the recovered recipe, and
reconstructs the command byte-for-byte, so duplicate or misplaced configuration
arguments still fail closed. Package-scoped tests also require a bounded Cargo
package name beginning with an alphanumeric byte.

Proof: the checked-in pipeline parses, resolves, persists, and decodes with all
four jobs; 33 run-plan tests are green; and the sandbox boundary admits every
supported offline recipe while rejecting unknown commands, option-shaped
package names, duplicate vendor frames, and misplaced compiler arguments.

## 2026-08-28 — truthful privacy holders reach the CLI

The durable privacy-request API and its full-system journeys already covered
agent data, authored Chat messages, and authored Issue titles, but the shipped
CLI exposed only the older agent-data status and direct-erasure shortcut. The
README was further behind: it still claimed Chat was absent from public privacy
requests and certificates. People therefore had to construct HTTP requests to
use two proven holders or retain any holder certificate.

`myelin privacy request erase <scope> --confirm` now projects the one durable
request contract for all three truthful scopes. Submission requires the
ordinary retry-stable idempotency key; status and certificate commands accept
only one canonical lowercase request UUID. Human output names the exact holder
scope, durable state, retry count, next command, per-holder erased record count,
key-unrecoverability proof, and certificate hash. JSON mode preserves the
complete server response. Unknown scopes, malformed identifiers, missing
confirmation, incomplete receipts, and unrecognised certificate shapes fail
locally or fall back to unmodified JSON instead of being optimistically
rendered.

The new TypeScript journey obtains a browser-approved session, proves an
unconfirmed command never reaches Edge, runs the real CLI binary to erase the
Chat holder, replays the same request identity, reads human status, and reads
both machine and human certificate forms. The existing three-scope privacy
lifecycle was rebuilt and rerun alongside it, so the CLI seam rests on the
same PostgreSQL-backed holder effects rather than a command-only fixture.

Proof: all 159 CLI library tests; strict all-target/all-feature CLI Clippy;
TypeScript typechecking; the rebuilt one-test CLI privacy journey; and all
three rebuilt privacy lifecycle journeys against the running platform.

## 2026-08-28 — durable workers own one explicit admission contract

Five deployed JetStream consumers previously combined a finite broker default
with an unrelated generic handler default. A service could tune its pull
window without tuning per-tenant unresolved retries, or vice versa, and the
canonical thresholds file described only request-shedding targets with human
reservations that make no sense after work has entered a durable queue.

`DurableWorkerAdmission` now validates one inseparable broker and handler
contract: maximum unacknowledged deliveries, maximum pull batch, and maximum
unresolved retries for one tenant. Refs projection, Notification routing,
governed agent triggers, CI dispatch triggers, and Git check projection each
load a named canonical row at startup, refuse missing or incoherent tuning,
apply it to the real JetStream consumer, and pass the same value into the only
production consumer builder. No builder retains an implicit production
default. The request-shedding table remains separate because a queued event
has neither a waiting caller nor a meaningful human lane to reserve.

Proof: all 221 Events library stories; all 30 threshold stories; all 161 Agent
Service and 96 CI Dispatch library stories; strict all-target/all-feature
Clippy across Events, Substrate, Refs, Notifications, Agent Service, CI
Dispatch, CI Control Plane, and Git; and the real PostgreSQL Refs and
Notification consumer suites, which assert the canonical per-tenant cap at
the live co-committing projector/router before applying durable effects; the
real JetStream/PostgreSQL/object-store CI dispatch journey; and both
PostgreSQL Git-check projection journeys, including constrained runtime-role
RLS and lossless redelivery.

## 2026-08-28 — Search no longer reports healthy while doing nothing

The `search` executable migrated a `search_index_directory` table, registered
zero event consumers, owned only in-memory Tantivy indexes, and exposed no
query handler. Its production bootstrap test called that useful merely because
the process stayed alive without a migration credential. The process was not
present in the running Fed topology, but its shell tests and migration-catalog
entry made the repository describe it as a deployable service.

That executable, its empty Substrate shell, its credential-liveness test, and
the isolated table-shape test are removed. The migration audit no longer calls
the unused directory table an authoritative production migration. Historical
applied rows remain harmlessly auditable; no destructive migration was added.
The Search crate remains the shared query/indexing library, and the running
bounded Git code-search product remains unchanged. A future Search service
must begin with a durable encrypted index, a real owner fetcher, durable event
intake, and a query surface before it can acquire a health endpoint.

Proof: all 327 Search library stories and every remaining all-feature Search
test target against the Fed durable dependencies; warning-free
all-target/all-feature Search and migration-audit compile and strict Clippy;
no remaining `search` binary, service-shell, migration-set, or fake
production-bootstrap reference.

## 2026-08-28 — self-hosted CI follows the workspace lockfile

Removing Search's unused executable changed the workspace package graph and
therefore the exact root `Cargo.lock` bytes, but the checked-in self-hosted CI
registry still selected the previous vendored dependency tree. The broad
backend run caught the mismatch at the real dispatch resolver: Myelin's own
push refused to arm because no trusted offline asset matched the new lockfile.

The complete workspace vendor tree has been rebuilt from `cargo vendor
--locked`, promoted through the existing content-addressed staging flow, and
repinned as one coherent identity in both `runner-assets.toml` and the sandbox
registry. The source and selection key are the exact root lockfile digest; the
runtime image reference is the canonical tree digest. No resolver fallback or
networked runner path was introduced.

Proof: the checked-in four-job pipeline now parses, resolves, snapshots, and
selects the rebuilt workspace vendor asset for every job; the staged tree and
its embedded lockfile match the committed pins; and the runner-asset digest
contract checks the registry against the manifest.

## 2026-08-28 — Search stops certifying stores it does not run

Removing the no-op Search executable exposed a larger self-certifying cluster
inside the library. Process-local cache maps, an in-memory KMS, a constant
`search_index` store descriptor, sealed byte vectors, and an in-memory Tantivy
registry were composed into “real erase,” backup, object-store, restore, and
telemetry gates. Every caller was another Search unit or drill; no deployed
worker, query process, privacy-request holder, durable index, or restore
maintenance command could reach any of them. Those tests could remain green
while the product held no Search projection at all.

The imaginary store boundary and its cache, key pin, layout, eraser, backup,
restore, residency, object-store, and telemetry models are removed together
with the tests that endorsed them. The reusable search core remains: typed
producer projections, canonical event indexing, ACL-conjoined full-text and
semantic query planning, consistency checks, Tantivy/vector algorithms, and
reindex mechanics. Search event envelopes still preserve personal-data
metadata, but the library no longer claims that metadata reaches a holder that
does not exist. A real holder now has one prerequisite: the same deployed,
durable encrypted index required by the product gap.

The same rule removes Search-only surge, freshness, filtered-ANN, projection-
promotion, and cross-cell simulators. They measured their own in-memory inputs
and then labelled the result production readiness. Their three canonical
threshold sections are gone too; operational tuning will return with the
deployed component that can actually observe it. The interactive
`SearchQuery` shed target remains because it describes a future request surface
rather than asserting that a worker exists.

Proof: all 207 remaining Search library stories and every remaining Search
test target against Fed's durable dependencies; all 27 focused threshold
stories; all-target/all-feature Search compile; no Search cache, KMS pin, store
descriptor, holder, backup, restore, synthetic load/freshness gate, or orphaned
threshold symbol remains; and the reduced workspace lockfile has a freshly
rebuilt, content-addressed offline Cargo vendor asset selected by self-hosted
CI.

## 2026-08-28 — transport capacity follows credential and principal class

Verified dispatch admission still began too late for four bounded resources.
Identity preparation, request-body collection, Git push and wire execution, and
large response materialization all had flat process caps. A burst of 300 valid
agent MCP posts proved the remaining problem: agents filled the 256-operation
identity backstop before principal resolution, and a browser-approved human
briefly received 503 even though the later dispatch lane was reserved.

Edge now prepares the method, path, query, headers, exact action, identity, and
tenant lane before it reads a body, then attaches the bounded bytes to that same
prepared request. It never parses or authenticates twice. Preparation reserves
capacity from the credential selector: the signed-token verifier requires the
`session` scheme exactly when the token carries the human-session purpose, so a
valid agent credential cannot select it and the caller-controlled run-class
header is irrelevant. Public and machine preparation have smaller caps inside
the general ceiling.

After preparation, verified principal kind owns classed request-body, Git push,
Git wire, and large-response permits. Machine and unauthenticated slow bodies
cannot consume the human reserve. Machine response permits live until the
client finishes or drops the body, not merely until the handler returns, and
every refusal before body collection closes the HTTP/1 connection explicitly.

The black-box overload journey now reruns against the same durable organization
with a unique agent name and observes concurrent probes through one immediately
handled promise. Its 300 real one-minute agent MCP posts all omit class hints;
the browser session stays strict-200 throughout and the machine lane recovers.

Proof: all 367 Edge library stories; all 21 real HTTP transport stories twice,
including forty incomplete machine uploads across four tenants, eight incomplete
public login bodies, and concurrent machine/human Git responses; strict
all-target/all-feature Edge Clippy; TypeScript typechecking; and the rebuilt
PostgreSQL-backed full-system overload journey.

## 2026-08-28 — every focused approval story carries its timer

The broad backend run found one Flow approval drill left behind by the durable
timer hardening. Its two stories asked for a seven-day signal wait without
supplying a timer wheel, so the production workflow context correctly failed
instead of parking. This was a fixture gap rather than a product shortcut: the
neighboring restart/remint stories already carry timers, and the PostgreSQL
dispatcher persists and fairly fires real deadlines.

The focused approval harness now carries the same timer store through each
fresh worker. Both approval and denial paths assert that exactly one unfired
timer exists on the run's partition at the precise first-drive time plus seven
days. The drill therefore cannot regain a misleading green result by merely
attaching an arbitrary timer context.

Proof: both focused multi-day approval cases; all five neighboring per-effect
and restart/remint cases; strict Clippy for the repaired all-feature test
target; and the live PostgreSQL dispatcher fairness story, which persisted and
fired five competing deadlines amid ordinary runnable work.

## 2026-08-28 — verified identity owns process dispatch admission

Edge's tenant lanes already derived traffic class from an authenticated
principal, but the process-wide machine pool ran first and trusted only two
optional request headers. Production defaults an omitted token-scheme header to
`agent`; a valid service request could therefore omit both headers, bypass the
48-slot machine pool, and enter the 64-slot general dispatch pool before Edge
discovered its real class. Coordinated traffic from several tenants could fill
that pool even though no individual tenant crossed its own bound.

Gateway request handling is now split at a real product boundary. Preparation
matches the route, verifies the capability and durable principal state,
resolves tenant and region, authorizes the exact action, and acquires the
per-tenant lane once. Only that prepared request reaches transport dispatch.
The transport admits service and agent principals to the machine pool from the
verified run class, then admits all prepared work to the general pool; neither
header participates in that process decision. Preparation has its own bounded
256-operation backstop, matching the configured identity-authz bulkhead, and
the handler never authenticates a request twice.

The former black-box overload story had a human bootstrap credential volunteer
`x-myelin-run-class: batch-ci`, so it could stay green while an actual agent
omitting the header bypassed the process pool. It now creates an external agent
through Myelin, starts a one-minute durable run, sends its governed MCP reads
with only the bearer, keeps a separately browser-approved human responsive,
and closes the run afterward. The transport regression drives 48 headerless
service requests across four tenants through real HTTP handling: the next
machine request gets 429 while an authenticated human still gets 200.

Proof: all 366 Edge library stories; all 19 HTTP transport integration stories;
strict all-target/all-feature Edge Clippy; TypeScript typechecking; and the
running TypeScript overload journey against the rebuilt PostgreSQL-backed Edge.

## known gaps (honest list, in priority order)

1. **erasure-restore is closed for three drilled scopes.** the
   agent-data and authored-Chat-message erasers write separate post-PIT ledger
   scopes. the maintenance command replays both through their production
   holders before a restored cell can reopen. agent data is drilled against a
   real dump; Chat now has the same real `pg_dump`/`pg_restore` proof, including
   a deliberately resurrected decryptable body. authored Issue titles have the
   equivalent real restore proof around their independent keys, bounded
   tombstoning, holder receipt, public requests, ledger replay, and fail-closed
   legacy-row handling. Git has not yet begun a truthful holder path and must
   not be exposed through privacy requests first.
2. **DSR has three truthful product slices, not full holder coverage.** durable
   submit/status/certificate is wired for `agent_data`, `chat_messages`, and
   `issue_titles`, with holder-specific proofs and black-box user journeys. the
   narrow scopes deliberately do not claim Chat drafts, read state, mentions in
   other people's content, Issue bodies/comments/custom fields, search
   projections, or Git. those holders remain absent rather than being
   represented by ceremonial receipts.
3. **Search indexing is not a deployed durable worker yet.** Refs,
   Notification, governed-agent, CI-dispatch, and Git-check NATS consumers now
   enforce named broker/batch/per-tenant admission rows. The former no-op
   Search shell is gone: there is no production index consumer, fetcher, or
   durable encrypted index backing, so there is deliberately no
   worker-admission row to bless.
   The former in-memory cache, KMS pin, store descriptor, eraser, and
   backup/restore drills are also gone; they cannot be mistaken for that
   production boundary.
   The existing `SearchQuery` shed row remains an interactive-surface target,
   not evidence that indexing is running.
4. **cross-product search is not surfaced yet.** the running product has bounded,
   authorization-filtered repository code search, but no Edge/CLI/browser surface over the
   Search service's issue, Knowledge, Chat, and CI projections.
5. **Chat reference cards do not cover every owner yet.** Edge, CLI, and browser now surface
   viewer-scoped Issue, Knowledge page, Git repository, Git pull-request, CI run, and Chat
   conversation cards, plus named private agent threads, through bounded owner queries, with
   content-free tombstones for denied or unavailable artifacts. Git comments
   now resolve to their exact discussion or diff location. Chat messages and
   their focused reply threads open exact permission-reconfirmed views. Git
   review decisions and other CI artifacts still retain their canonical link
   rather than a rich card because their durable owner projectors are not
   composed. Event-driven cache invalidation and live card updates also remain unwired.
6. **Issues has no production live-board transport yet.** the browser offers durable,
   paged issue views and mutations, but there is no Edge board-op stream, authenticated
   resume/snapshot boundary, or reconnecting board client. the former in-memory facade
   was removed rather than counted as shipped behavior.
7. **CI has no user-visible cache yet.** the old process-local namespace is gone;
   the historical `cache_entry` table has no store, pipeline syntax, runner
   restore/save operation, retention policy, or full-system journey. the eventual
   design must derive its namespace from the durable run trust stamp.
8. **stale branch archaeology:** `codex/*`, `claude/*`, `wip/2*`–`wip/35*`
   (CT-007 sandbox slices) sit on a disjoint history root ("founder source
   snapshot") with no common ancestor with main. any useful content must be
   mined as diffs. left in place, treated as archive.
9. **HYOK/BYOK has no production boundary yet.** The unwired `KeyOrigin`
    model was removed after its BYOK path proved to be ordinary platform-key
    wrapping with an unused customer-path string. No running configuration
    selects an origin for a product data class and Search projections do not
    carry that decision into index admission. The former SRCH-D10 drill was
    removed because its supposed HYOK inspection returned `false`
    unconditionally. A replacement must start with a real customer-key
    adapter, attempt the ordinary indexing path, and inspect every actual
    derived store.
