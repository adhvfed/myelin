# worklog

A running log of autonomous product work: what changed, why, and what the
evidence was. Newest entries first. Every entry names its proof — if a claim
here has no test or drill behind it, treat it as wrong.

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
share the subject key and all three report their own deletion counts.

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

## known gaps (honest list, in priority order)

1. **erasure-restore is closed for the wired path, open for the rest.** the
   agent-data erase now writes the post-PIT ledger and the re-erase pass is
   drilled against a real dump. the maintenance command and runbook replay it
   through the production holder and restore the absorbing processing block.
   remaining: the library-level erase paths (chat, issues, git crypto-shred)
   still do not write the ledger because nothing wires them to a user surface
   yet (gap 2).
2. **DSR has one truthful product slice, not full holder coverage.** durable
   submit/status/certificate is now wired for the real agent-data holder and
   exercised through Edge. chat/issues/git erasure flows still exist as
   in-memory tested code only, so they are deliberately absent from the public
   certificate rather than being represented by ceremonial receipts.
3. **multi-tenant machine storms can still exhaust the general dispatch
   pool.** the human lane holds per tenant and against well-behaved machine
   traffic; a coordinated cross-tenant storm of requests that lie about
   their class is bounded only by the flat caps. fixing this properly means
   authenticating before dispatch admission (an edge refactor).
4. **asynchronous worker admission is not explicit yet.** search, refs,
   notification, and automation work arrives through durable NATS/SQL queues;
   queue bounds and worker concurrency provide backpressure, but the matching
   thresholds.toml shed rows are targets rather than production enforcement.
5. **cross-product search is not surfaced yet.** the running product has bounded,
   authorization-filtered repository code search, but no Edge/CLI/browser surface over the
   Search service's issue, Knowledge, Chat, and CI projections.
6. **Chat reference cards do not cover every owner yet.** Edge, CLI, and browser now surface
   viewer-scoped Issue, Knowledge page, Git repository, Git pull-request, CI run, and Chat
   conversation cards, plus named private agent threads, through bounded owner queries, with
   content-free tombstones for denied or unavailable artifacts. Git comments
   now resolve to their exact discussion or diff location. Chat messages and
   their focused reply threads open exact permission-reconfirmed views. Git
   review decisions and other CI artifacts still retain their canonical link
   rather than a rich card because their durable owner projectors are not
   composed. Event-driven cache invalidation and live card updates also remain unwired.
7. **Issues has no production live-board transport yet.** the browser offers durable,
   paged issue views and mutations, but there is no Edge board-op stream, authenticated
   resume/snapshot boundary, or reconnecting board client. the former in-memory facade
   was removed rather than counted as shipped behavior.
8. **CI has no user-visible cache yet.** the old process-local namespace is gone;
   the historical `cache_entry` table has no store, pipeline syntax, runner
   restore/save operation, retention policy, or full-system journey. the eventual
   design must derive its namespace from the durable run trust stamp.
9. **stale branch archaeology:** `codex/*`, `claude/*`, `wip/2*`–`wip/35*`
   (CT-007 sandbox slices) sit on a disjoint history root ("founder source
   snapshot") with no common ancestor with main. any useful content must be
   mined as diffs. left in place, treated as archive.
