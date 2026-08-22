# worklog

A running log of autonomous product work: what changed, why, and what the
evidence was. Newest entries first. Every entry names its proof — if a claim
here has no test or drill behind it, treat it as wrong.

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
