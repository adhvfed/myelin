# Sixty-six directions for the next Myelin explorations

Status: exploration ledger, written after the 2026-08-15 full-system checkpoint.

This is a menu of investigations, not a claim that all sixty-six are currently broken and not a
priority ordering. Each exploration should begin by tracing the current production path; the work
since the 2026-08-10 review repaired many of that review's findings. Prefer deleting a stale seam
over preserving it, and prefer a black-box TypeScript story or a live-storage integration story
over a source-shape assertion.

“Previous ledger” below means the immutable Git history: an abbreviated commit and a readable form
of its subject. Contract numbers refer to `contracts/contract-index.md`. Review references such as
`02-H4` refer to the files in `~/Planning/myelin/2026-08-10-review/`. Together they show why a path
exists, what has already been tried, and which promise must remain true while it changes.

## A. Starting a software organization

### 1. Make the first hour one continuous journey

**Approach.** Follow a completely empty tenant from browser sign-in through project and repository
creation, first issue, first push, first CI result, and first agent-assisted task. Remove every UUID,
environment variable, service restart, and unexplained wait from that path, then express it as one
TypeScript system story whose prose describes the person's intent rather than the endpoints.

**Previous ledger.** This extends `133746ec` (“Make first project onboarding self-service”),
`5beab430` (“Let teams start projects from Issues”), and `89845dab` (“Activate external agents from
the CLI”); it is the user-level synthesis that those individually useful entries did not attempt.

### 2. Design a credentialless migration concierge

**Approach.** Explore browser-approved, short-lived imports from GitHub, Linear, Slack, Jira, and
Notion without asking an operator to paste API keys into Myelin or an agent prompt. Start with one
provider and model authorization, preview, resumability, provenance, revocation, and secret custody
as a reusable connector contract rather than five provider-specific shortcuts.

**Previous ledger.** `9bfa8f5d` (“Bring resumable issue imports to the CLI”) and `9988f566`
(“Keep CLI credentials in the OS keyring”) established safe local ingredients; this exploration
advances the product vision from file import toward no-key-mess organizational adoption.

### 3. Give operators a useful `myelin doctor`

**Approach.** Build a read-only diagnostic command that checks the selected context, Edge reachability,
browser-approved session, Git credential helper, repository visibility, CI readiness, agent runtime,
and dependency health. Its output should distinguish “not configured,” “temporarily unavailable,”
and “permission denied,” and offer an exact next command without printing credentials.

**Previous ledger.** This follows `5195778e` (“Hide expired CLI login state”), `d2946a5b` (“Keep
branch protection guidance actionable”), and contract 1.3’s liveness/readiness distinction by
turning scattered good error behavior into one end-user recovery surface.

### 4. Make reusable team recipes a first-class product object

**Approach.** Explore a bounded template that can create a project, repository policy, CI definition,
issue defaults, agent registration, and automation bindings as a previewable plan with retry-safe
steps. A recipe must reference platform capabilities and canonical objects, never embed third-party
secrets, opaque tenant IDs, or unchecked executable setup scripts.

**Previous ledger.** `34b6b49c` (“Require storage for project onboarding stories”), `9023ac09`
(“Register issue metadata during operator bootstrap”), and contract 3.2 provide the durable pieces;
this would connect them into repeatable organizational startup.

### 5. Unify browser, desktop, CLI, and Git context

**Approach.** Specify one visible model for active organization, project, repository, and acting
identity across every client. Explore deep links that can select a context with confirmation, while
keeping stored credentials bound to their issuing Edge and ensuring a copied link can never smuggle
authority or silently change the active mutation destination.

**Previous ledger.** `b2e23d45` (“Add named CLI contexts”), `17d6dcaf` (“Make project context drive CLI
work”), and `43790429` (“Remember the issuing Edge after CLI login”) solved this in layers; the next
step is parity and predictability across clients.

### 6. Turn ambiguous onboarding outcomes into durable receipts

**Approach.** Audit every multi-step onboarding mutation for the response-loss case: the server may
have committed while the client saw an error. Return addressable operation receipts and expose a
status/reconcile command so retrying never depends on optimistic wording or human inspection of
several lists.

**Previous ledger.** `b66b8ab0` (“Make CLI token exchange response-loss safe”), `94c300fa` (“Return
addressable receipts from agent mutations”), and `d07d996d` (“Keep trigger registration retries
stable”) are precedents for making uncertainty queryable rather than rhetorical.

## B. Identity, authority, and delegation

### 7. Prove resource-bound delegation end to end

**Approach.** Exercise repository-, project-, channel-, and artifact-scoped caveats from creation to
the signed run identity and again at each tool adapter. Add negative full-system stories showing that
a tool selected for one repository cannot enumerate or mutate another even when the human delegator
could, and expose an explanation of the effective intersection to the owner.

**Previous ledger.** `c30bdbfd` (“Enforce automation delegation caveats”), `27a5cbad` (“Require issued
identity for MCP routing”), review `03-H1/H3`, and contracts 4.5 and 4.7 define the lineage and the
security claim that still deserves system-level proof.

### 8. Make policy explanation part of ordinary delegation

**Approach.** Explore a shared “why can this agent do this?” projection that names the tool grant,
delegator relationship, tenant ceiling, caveat, freshness watermark, and approval rule without
leaking hidden object existence. Use the same typed explanation in CLI, GUI, audit records, and
denial diagnostics so policy behavior is debuggable without reading several stores.

**Previous ledger.** `610938ff` (“Honor relationship consistency from one snapshot”), `0253d0a4`
(“Honor relationship watermarks at the service boundary”), and contract 4.4’s `explain` seam supply
the consistency and vocabulary this surface should reuse.

### 9. Revisit agent ownership as a collaboration model

**Approach.** The creator-only lifecycle rule is a safe floor, but organizations may need explicit
co-owners, transfer, emergency suspension, and separation between use, edit, and retire. Model those
relationships rather than broadening tenant-wide authority, and prove that retirement remains
absorbing, attributed, and incapable of being smuggled through a lower privilege.

**Previous ledger.** `30cf4c9e` (“Keep agent ownership with its creator”) repaired review `01-H1`;
this exploration treats that repair as the base for a deliberate multi-human ownership product,
using contract 4.2 rather than handler-local exceptions.

### 10. Make run-credential closure independently observable

**Approach.** Trace every terminal path—success, denial, timeout, cancellation, worker crash, client
disconnect, agent suspension, and storage outage—and ensure token teardown is retried or visibly
quarantined. Provide safe metrics and audit outcomes that identify the run and reason without ever
exposing a bearer, JTI, or provider secret.

**Previous ledger.** `22475513` (“Make run token teardown explicitly fallible”), `9b6059c2` (“Close
credentialless agent runs atomically”), and contract 4.7 move teardown from a destructor assumption
toward an operable state transition; this would close the observational seam.

### 11. Harden browser-approved device authorization as a public service

**Approach.** Measure abuse limits by source, tenant hint, and outstanding authorization count;
consider proof-of-work or progressive delay only after simpler bounded storage and fair rate limits.
Unify expired and unknown user-code behavior, test collisions under pressure, and ensure legitimate
CLI recovery remains humane.

**Previous ledger.** `4f6c4a77` (“Bound unauthenticated CLI login starts”), `fc37f059` (“Add
verifier-bound CLI device authorization”), and review `01-M7/L4` identify both the established
protocol and the remaining public-boundary questions.

### 12. Audit every machine identity through one authority vocabulary

**Approach.** Inventory SSH keys, deploy keys, PATs, CI job credentials, agent runs, and service
identities, then normalize audience, purpose, subject, expiry, revocation, and resource binding.
Replace route-specific boolean interpretations with typed authority contexts and add cross-door tests
that a credential accepted at one intended surface is refused at its twins.

**Previous ledger.** `4f498d6c` (“Persist the typed CI credential binding”), `7434b273` (“Make checked
Git routes structural”), `a190d44b` (“Confine run credentials to MCP governance”), and contract 4.1
show the converging design this audit should finish.

## C. Agent execution and governance

### 13. Model replay around effects, not provider call IDs

**Approach.** Reconfirm that every model turn and tool effect has a platform-minted semantic identity
derived from run, workflow position, canonical tool, and arguments. Test changing provider call IDs,
changing read results, process death after apply, and multiple approvals; replay should reuse the
recorded observation or effect receipt without duplicating a mutation or poisoning the run.

**Previous ledger.** `1e38605d` (“Journal hosted tool effects across resumes”), `41813f15` (“Bind
agent journal operations with typed context”), and review `03-H2/H4` are the direct history; contract
9.1 supplies the per-effect idempotency rule.

### 14. Generalize governed approval beyond `git.merge`

**Approach.** Add a second genuinely different gated effect—such as environment deployment or
Knowledge publication—to force approval identity, presentation, audit, wake-up, expiry, and apply
semantics out of Git-specific branches. The tool definition should carry the schema needed to render
and audit an approval, and unsupported gated tools should be impossible to register.

**Previous ledger.** `3357beaf` (“Decompose governed approval routing”), `789d2f63` (“Make MCP audit
outcomes structurally complete”), review `01-M1/03-M2`, and contracts 8.1–8.2 explain why a second
effect is the best architectural test.

### 15. Make approval eligibility a live, explainable set

**Approach.** Explore approval cards backed by the same `list_subjects` result used at decision time,
with membership changes, self-approval prohibition, quorum, delegation, expiry, and stale inbox copies
handled explicitly. A user should be able to see why they can decide, and an obsolete card should
converge without a silent failure.

**Previous ledger.** `94abf55b` (“Make hosted agent approvals actionable”), `83110418` (“Converge
terminal approvals after missed wakes”), `eed14f0c` (“Keep approval storage outages fail closed”),
and contract 4.4 provide the durable and authorization foundations.

### 16. Give governed reads first-class audit and privacy semantics

**Approach.** Verify that successful, denied, and failed reads produce bounded, typed audit outcomes
without copying returned source material into the audit stream. Add owner-facing history that can
answer which repositories, issues, chats, pages, and CI logs an agent consulted, while respecting
erasure, retention, and hidden-object rules.

**Previous ledger.** `b01435a9` (“Audit governed reads as agent work”) answered review `03-H3` at the
routing layer; `789d2f63` and contracts 8.2 and 10.6 suggest the next step is a complete, usable
audit product rather than merely emitted records.

### 17. Collapse run termination and budget settlement into one state machine

**Approach.** Enumerate every terminal and indeterminate combination of workflow, run token, cost
reservation, pending approval, trace write, and automation firing. Introduce a single reconciliation
model with monotonic states and retryable commands, then prove crash points with live Postgres tests
so no reservation or approval can remain stranded forever.

**Previous ledger.** `cc07097a` (“Release budgets for terminal agent work”), `8d4a2827` (“Keep
settlement outages from crashing agent workers”), `2e18d23b` (“Stop disabled automations atomically”),
and contracts 9.5 and 11.7 expose the presently distributed lifecycle.

### 18. Build an adversarial model-provider simulator

**Approach.** Replace deterministic happy-path fixtures in reliability tests with a scripted provider
that changes call IDs, reorders tool calls, streams partial output, repeats a request, reports invalid
usage, times out after charging, and returns malformed structured arguments. Keep it test-only and
drive the production host boundary so provider nondeterminism cannot be accidentally normalized away.

**Previous ledger.** `34887ed6` (“Keep the scripted model out of production workers”), `13b2dc95`
(“Keep MCP test doubles out of production”), review `03-H4`, and the test-quality review’s warning
about deterministic call IDs make this a purposeful test double rather than product scaffolding.

## D. Automations and durable workflow

### 19. Give poison firings an operator-owned recovery loop

**Approach.** Inspect the existing quarantine behavior and add bounded retry classification, a durable
dead-letter record, safe event-envelope diagnostics, replay after correction, and an owner-visible
terminal explanation. One malformed firing must neither crash a worker nor disappear into metrics;
replay must preserve its original run identity.

**Previous ledger.** `6013fa77` (“Keep poison firings from stopping the worker”), `3ece507d` (“Keep
hosted agent failures diagnosable”), review `05-HIGH`, and contract 2.4’s Retry/NonRetryable split are
the starting point for the operability story.

### 20. Make automation fanout fair under hostile density

**Approach.** Replace tenant-wide event starvation with per-owner quotas, paged evaluation, and a
bounded scheduler that lets every eligible binding make progress. Test 1,001 noisy bindings beside
one quiet owner’s rule and verify admission errors occur at creation or on the offending binding,
never by dropping the event for everybody.

**Previous ledger.** `06218051` (“Keep automation fanout fair and bounded”) addressed review `05-MED`;
this exploration should validate that promise at scale and connect it to contract 3.6’s bounded
dispatch rather than a magic global cap.

### 21. Separate replay time from deadline time in the type system

**Approach.** Inventory every expiry, lease, credential TTL, SLA, and approval comparison inside
workflow activities. Use distinct types or APIs for journaled logical time and live wall time, make
the intended choice review-visible, and test a process sleeping beyond each deadline before replay.

**Previous ledger.** `682b17d9` (“Keep approval deadlines on live time”) repaired the concrete review
`05-HIGH` case; contract 9.2 still makes time a high-fan-in seam, so structural prevention is worth
exploring beyond that fix.

### 22. Productize the schedule-and-run-job idiom

**Approach.** Implement one reusable workflow component that reserves cost, dispatches a CI or agent
job, releases the runtime, waits for an idempotent completion signal, and reconciles cancellation or
lost delivery. Exercise it with both CI and agent work so the contract is real shared machinery,
not two implementations with similar comments.

**Previous ledger.** `18edbb77` (“Co-commit trigger firings with agent workflows”), `80833ced` (“Bind
durable dispatch replay as one identity”), and contracts 9.2 and 11.7 name this exact shared seam.

### 23. Make automation dry-run preview the real effect plan

**Approach.** Let an owner feed a representative event into a draft automation and see matcher
evaluation, selected agent, effective authority, estimated budget, approval points, and proposed
effects without mutating durable work. The preview should call the same compiler and governance
interfaces as execution and label inherently model-dependent results as provisional.

**Previous ledger.** `8329fac7` (“Let automations express bounded event filters”), `44573dc4` (“Make
automation rule failures diagnosable”), and contract 8.7 provide the bounded predicate and dry-run
promises that can be joined into a trustworthy authoring loop.

### 24. Offer safe automation templates with activation checks

**Approach.** Explore templates such as red-build triage, stale-review reminder, incident runbook
linking, and release-note drafting. Instantiation should resolve canonical resources, show required
tools and approvals, validate current visibility, start paused, and refuse decorative or unsupported
caveats before any event can fire.

**Previous ledger.** `aa0f4622` (“Give automation owners a web workspace”), `f8eca7a9` (“Scope CI
automations to one repository”), and `617c9d81` (“Require durable storage for automation stories”)
make templates a usability layer over real durable behavior rather than canned demos.

## E. Git and CI as one delivery system

### 25. Make repository creation one recoverable transaction

**Approach.** Re-evaluate the filesystem-plus-authorization creation sequence under process death and
concurrent creators. Prefer a durable creation operation with a unique owner, staged repository,
atomic promotion, and reconciler; prove that no principal can inherit administration of a repository
whose slug was later claimed by somebody else.

**Previous ledger.** `2d0a9a0f` (“Make repository creation retry safe”), `898cf060` (“Make first
repository onboarding truthful”), and review `02-H5` show that sequential retry safety is only one
part of the creation invariant.

### 26. Close the lifecycle of quarantined Git objects

**Approach.** Follow rejected edits and pushes from quarantine creation through policy evaluation,
promotion, cleanup, crash recovery, and disk accounting. Add a full-system secret-rejection story
that proves the object is not readable by OID, plus an operational sweep whose safety is based on
reachability and durable operation state rather than age alone.

**Previous ledger.** `3a9206bd` (“Quarantine file edits before promotion”), `01b9e957` (“Leave failed
Git restores cleanly retryable”), and review `02-H3` establish why ref rejection is insufficient if
objects have already escaped quarantine.

### 27. Establish one branch-policy decision boundary

**Approach.** Inventory smart HTTP, web edits, agent writes, merges, merge queue, restore, and admin
operations, and route every ordinary ref transition through one typed policy context. Keep explicitly
privileged recovery paths separate and audited. Contract tests should add a new protected pattern
once and demonstrate identical behavior through every door.

**Previous ledger.** `4df72f02` (“Honor branch protection on file edits”), `0aea4abd` (“Carry
validated Git ref identities into receive”), review `01-H2/02-H2`, and contract 5.9 point toward one
decision seam rather than synchronized local rules.

### 28. Finish retry identity for pull-request collaboration

**Approach.** Bind PR creation, review start, inline comment, review submission, merge, and update
commands to caller, route, repository, payload fingerprint, and stable client idempotency key.
Return the original receipt after response loss, reject semantic key reuse, and make double-clicks
boring in both web and CLI system stories.

**Previous ledger.** `9a51849a` (“Bind pull request command replay identity”), `18fed9f0` (“Bind durable
review comments as operations”), review `02-M4/M7`, and contract 9.1 supply the partial convergence
and the reusable command identity rule.

### 29. Exercise the merge queue as the delivery spine

**Approach.** Create a black-box story in which two pull requests contend, checks supersede by run
attempt, an untrusted-fork success remains neutral, branch state moves, one run is cancelled, and the
surviving PR merges exactly once. Assert user-visible queue explanations as well as database-safe
outcomes.

**Previous ledger.** `f23c3728` (“Unify durable check status projection writes”), `64cd116f` (“Wire
and verify the external CI lifecycle”), and contracts 5.9 and 9.4 define this cross-service promise;
the exploration would test it as one product rather than isolated mechanisms.

### 30. Complete resumable, erasable CI log delivery

**Approach.** Join live firehose frames, resume cursors, sealed content-addressed segments, byte-range
indexes, jump-to-failure references, subject-key erasure, and authorization into one externally driven
story. Test disconnect/reconnect, duplicate frames, segment sealing failure, trust-tier isolation,
and a user erasure without making the rest of the run unreadable.

**Previous ledger.** `531789ab` (“Make CI log ingestion errors explicit”), `ff067136` (“Make CI
artifact reference minting fallible”), and contracts 3.5, 11.4, and 11.8 describe the pieces whose
assembled behavior matters to users.

## F. References, search, and durable knowledge

### 31. Finish every reference producer and removal path

**Approach.** Trace Chat, Issues, Knowledge, Git, and CI from canonical content changes to co-committed
`refs.edge.created` and `refs.edge.removed` events. Editing, deleting, moving, erasing, or rewriting a
source must remove or replace its edges monotonically. Delete any producer glue that remains test-only
after the chosen product scope is explicit.

**Previous ledger.** `9f5b631d` (“Unify reference edge vocabulary”), `874220a0` (“Project typed
reference edges atomically”), `5ef3594c` (“Project knowledge references from durable blocks”), review
`02-H4`, and contract 5.4 are the direct lineage.

### 32. Make reference reindex rebuild the production store

**Approach.** Build an operator-visible reindex job that asks owners for bounded snapshots, feeds the
same live projector, records cursors and failures, and replaces a scoped durable projection without
mixing generations. Test a blank Postgres edge table and a deliberately corrupt projection rather
than an in-memory twin.

**Previous ledger.** `9644df29` (“Make reference projector stories self-describing”), `a4f0e0ef`
(“Require storage for durable ledger stories”), review `02-H4`, and contracts 2.6 and 5.8 require
rebuildability to be production behavior.

### 33. Give reference tombstones monotonic version authority

**Approach.** Introduce an owner revision or event sequence into edge upserts/removals so delayed
delivery cannot resurrect a removed edge. Specify move, erase, reindex, and cross-region conflict
semantics, then exercise deliberately reordered NATS delivery against the real projector.

**Previous ledger.** `eaef5560` (“Strengthen durable reference identities”), `a1dff07a` (“Keep
reference edges in their home region”), review `02-M3/L2`, and contract 2.3’s aggregate ordering show
both the prior identity work and the missing monotonic dimension.

### 34. Prove permission push-down at hostile scale

**Approach.** Populate tens of thousands of visible and hidden artifacts, dense Chat membership, and
hot backlinks, then measure list/search/backlink behavior with one consistency watermark. Fail closed
without post-filter truncation, N+1 checks, hidden counts, or an authorization scan cap that an
attacker can fill.

**Previous ledger.** `43d3bf29` (“Name reference visibility queries”), `4f757656` (“Page hot
backlinks without rescanning”), `610938ff` (“Honor relationship consistency from one snapshot”),
review `01-L6/02-M2`, and contracts 4.3 and 5.3 define the bar.

### 35. Make cross-cell references honest before making them clever

**Approach.** Start with one artifact in a remote home cell and prove that only its PII-free pointer
crosses, while permission checking and rendering remain home-cell operations. Specify unavailable,
moved, erased, split-brain, and residency-denied states; avoid a global cache that quietly becomes a
second source of content.

**Previous ledger.** `a1dff07a` (“Keep reference edges in their home region”), `7be426ce` (“Follow
live tenant placement in Refs”), `5aad7dea` (“CrossCellPointer bridge resolution live”), and contracts
5.2 and 12.6 establish the cell-local principle this exploration should prove.

### 36. Let agents research with attributable, permission-safe citations

**Approach.** Explore an agent search flow that returns canonical references plus bounded snippets,
then records which sources supported the final durable trace. Visibility must be evaluated for the
delegator and narrowed by delegation; later permission loss or erasure should tombstone the citation
without copying source text into the trace or audit log.

**Previous ledger.** `dab4df96` (“Expose delegated source context through MCP”), `eb76916a` (“Read
Knowledge pages from canonical refs”), `d5eb761d` (“Test traversal against repository directories”),
and contracts 6.1–6.2 and 8.8 connect RAG to durable provenance.

## G. Human collaboration

### 37. Evolve Chat privacy from project membership, not channel flags

**Approach.** Model channel read/post authority as live project and explicit membership relations,
including private rooms, archived rooms, agent delegation, membership removal, and historical message
visibility. Test list, read, post, search, notifications, references, and MCP together so no secondary
surface turns a private conversation into an existence oracle.

**Previous ledger.** `8a494693` (“Bind public conversations to project access”), `387bb3c7` (“Keep
private Chat context out of backlinks”), review `01-M4`, and contract 4.9 provide the intended
relationship model and two already-secured secondary doors.

### 38. Make issue workflows self-service without becoming unbounded

**Approach.** Let project owners define a small typed set of issue types, states, transitions, and
required fields, with server-owned keys and migration previews. Reuse `CaveatContext` for transition
authorization and keep board/query compatibility explicit; reject scripts and arbitrary expressions
outside the bounded shared query language.

**Previous ledger.** `9023ac09` (“Register issue metadata during operator bootstrap”), `5beab430`
(“Let teams start projects from Issues”), and contracts 4.2 and 13.3 turn today’s default metadata
into a coherent next layer of project autonomy.

### 39. Prove one content model from Rust to the editor

**Approach.** Compile the canonical content parser/renderer to WASM and run a shared corpus through
Rust, browser editing, import, reference extraction, and round-trip rendering. Unsupported imported
nodes should produce an explicit lossy report; structured references must survive editing without
being flattened into stale display text.

**Previous ledger.** `2842135d` (“Keep structured references intact while editing”), `4761e384`
(“Let humans and agents link living docs”), and contracts 13.1–13.2 specify the frozen model this
cross-language proof should enforce behaviorally.

### 40. Turn linked work into living project context

**Approach.** Explore a project view that gathers issues, pull requests, CI runs, conversations, and
Knowledge pages through canonical edges, with per-viewer projections and clear stale/tombstone states.
Let humans and agents add attributed links, but derive summaries live so the view does not become a
new denormalized authority or privacy holder by accident.

**Previous ledger.** `c3a967ec` (“Let people link related Myelin work”), `b6b8cfb4` (“Show linked work
in pull request context”), `174de926` (“Link Knowledge work through canonical refs”), and contract 5.2
form the existing graph-shaped foundation.

### 41. Make the inbox a complete work-decision surface

**Approach.** Unify notifications, automation approvals, agent-effect gates, review requests, SLA
escalations, snooze, unread state, and pagination around addressable items and typed actions. Decisions
must surface conflicts and outages, stale copies must converge, and bulk operations must preserve
per-item authorization and idempotency.

**Previous ledger.** `8ad47675` (“Make notification inbox items addressable”), `0f584749` (“Let the
unified inbox load every page”), `95ad1827` (“Surface automation approvals in the shared inbox”),
review `06-HIGH`, and contract 7.1 show the feature’s gradual expansion.

### 42. Give review conversations durable command semantics

**Approach.** Treat review creation, comments, replies, resolution, submission, dismissal, and
notification completion as retry-safe commands with stable receipts. Bind comments to content
snapshots or rebased line anchors, preserve agent authorship structurally, and make edited or vanished
diff context legible rather than silently relocating a conversation.

**Previous ledger.** `18fed9f0` (“Bind durable review comments as operations”), `af4156d3` (“Clarify
durable review thread context”), `c2802711` (“Ship snapshot-pinned Git blame”), and contract 5.7
provide the operation and anchoring vocabulary.

## H. Privacy, durability, and security

### 43. Drive one real DSR through every agent-data holder

**Approach.** Submit an owner-authorized erasure through the production GDPR entry point and verify
fanout reaches durable traces, model replay steps, tool-effect journals, workflow history, audit-safe
pseudonyms, and suppression markers. Kill the coordinator between holders and resume it, producing an
addressable certificate instead of relying on direct store calls in separate tests.

**Previous ledger.** `9d364900` (“Expose self-service agent data erasure”), `e3bbede4` (“Erase every
subject-owned agent replay record”), `7f2be341` (“Make agent-data erasure resumable”), review `04-H1/H2`,
and contracts 10.1 and 10.4 identify the assembled claim.

### 44. Preserve ciphertext as evidence after shredding its key

**Approach.** In a live-storage test, copy a valid encrypted trace or log segment, erase the subject,
and prove the retained ciphertext cannot be opened while other subjects remain readable. Also assert
the ciphertext does not contain chosen plaintext and that a retry, restore, or new worker cannot
recreate the destroyed key.

**Previous ledger.** `a4d99358` (“Require encrypted agent traces”), `9d30e1da` (“Check durable KMS
failures in integration stories”), `3cd68b64` (“Remove ceremonial plaintext counter”), review
`07`’s crypto-test gap, and contracts 10.8 and 11.4 define the evidence that matters.

### 45. Model KMS concurrency as state transitions

**Approach.** Specify mint, read, cache refresh, rotate, destroy, restore, and offboard as a small
state machine, then test interleavings across two processes and injected transaction failures. A
destroyed key must be non-resurrectable, rotation publication must follow commit, caches must fail
closed, and all key material should be zeroized on drop.

**Previous ledger.** `5474ccfd` (“Zeroize in-memory KMS keys”), `a088a364` (“Make KMS state
transitions atomic and fallible”), `237d3f97` (“Publish durable KMS rotations only after commit”),
`e9a5a81b` (“Fail closed when the KMS read cache is unavailable”), and review `04-M1/M2` show the
progression toward this model.

### 46. Make restriction and retention observable product behavior

**Approach.** Explore owner/admin controls and status views for restriction, retention, legal hold,
and erasure eligibility across search, analytics, agents, notifications, exports, and immutable
history. Centralize the effective policy calculation, give every refusal a lawful and technical
reason, and test that lifting restriction does not resurrect erased material.

**Previous ledger.** `bef3e210` (“Blind agent privacy subject locators”), `fa330128` (“Make agent
result availability explicit”), and contracts 10.1, 10.5, 10.7, and 11.6 frame privacy as ongoing
operability rather than a delete endpoint.

### 47. Make restore verification a whole-product survival drill

**Approach.** Restore to a real point-in-time offset, reconcile blob and event cursors, reapply the
erasure ledger, rebuild derived search/reference stores, and then run a narrow TypeScript journey
covering Git, Issues, CI, Chat, Knowledge, and an agent result. Report exact recovery gaps and keep
the restored environment isolated from live outbound providers.

**Previous ledger.** `01b9e957` (“Leave failed Git restores cleanly retryable”), `307b6687` (“Use
durable KMS engine in kill-9 writer”), and contracts 2.6, 10.8, and 11.5 define restoration as more
than a database boot check.

### 48. Make tamper-evident audit externally witnessable

**Approach.** Complete the path from outbox-authored audit record through per-tenant hash chain,
Merkle inclusion proof, periodic external witness, verification CLI, and eDiscovery bundle. Test
omission, reordering, duplicate delivery, witness outage, key rotation, and redaction without putting
personal payloads into the durable proof structure.

**Previous ledger.** `789d2f63` (“Make MCP audit outcomes structurally complete”), the existing
GDPR audit drills noted in review `07`, and contracts 10.6–10.7 supply the records and proof contract;
the exploration would make them independently verifiable by an operator.

## I. Multi-tenant operations and scale

### 49. Prove multi-replica leases with unique worker identity

**Approach.** Run two real host or dispatch processes against one database, pause each at lease
boundaries, expire and reclaim work, and demonstrate that fencing—not merely a content-addressed
run ID—prevents stale completion. Derive worker identity per process and make ownership visible in
diagnostics without tying correctness to hostnames.

**Previous ledger.** `fcfb0566` (“Consolidate durable scheduler claim fences”), `14b5c660` (“Remove
in-memory scheduler twins”), review `05-MED`, and contract 2.4’s durable consumer discipline make
this the next honest concurrency proof.

### 50. Budget connections as a compositional resource

**Approach.** Map every service’s maximum outer transaction and nested storage acquisition, then
remove hidden second-pool calls or reserve capacity structurally. Add a low-pool system profile and
concurrent trace/KMS, authorization, and projection stories that fail quickly and explicitly instead
of deadlocking behind all checked-out connections.

**Previous ledger.** `31de617c` (“Bound database pools in the local full stack”), `da745c82` (“Release
journal transactions before replay decryption”), `d1d2d40d` (“Keep OLTP pool failures explicit”),
review `04-H3`, and contract 11.1 expose pool capacity as a cross-layer invariant.

### 51. Verify graceful drain across the assembled service graph

**Approach.** Send termination while HTTP requests, Git pushes, outbox relays, CI jobs, workflow
activities, agent tool calls, and notification deliveries are in flight. Each component should stop
admitting work, finish or release bounded claims, revoke ephemeral authority where required, flush
durable outcomes, and reach readiness/liveness states that an orchestrator can understand.

**Previous ledger.** `22475513` (“Make run token teardown explicitly fallible”), `f8c64b3f` (“Keep
sandbox process registries recoverable”), and contract 1.1’s graceful-drain promise invite a
full-system proof rather than independent shutdown handlers.

### 52. Admit new tenants without restarting consumers

**Approach.** Create and place a tenant after every service is already running, then verify event
consumers, refs, search, notifications, automations, and holders begin serving it without losing the
events emitted during registration. Prefer wildcard-free dynamic durable bindings or a controlled
registry reconciler with replay over boot-time snapshots.

**Previous ledger.** `9023ac09` (“Register issue metadata during operator bootstrap”), review
`02-M14`, and contracts 1.4, 2.4, and 12.3 expose the tension between dynamic tenancy and explicit
consumer subject whitelists.

### 53. Make residency attestation inspect the running data plane

**Approach.** Have every live store, queue, runner, cache, log tier, and outbound adapter report
signed placement evidence tied to deployed configuration and current tenant state. Aggregate those
claims into a CLI/API attestation that can show unavailable or contradictory components instead of
serializing a precomputed success fixture.

**Previous ledger.** `5afc9e18` (“Handle attestation serialization failures in tests”), `d2a5f92b`
(“durable control-plane placement registry + the placement invariant as a real DB trigger”), and
contract 12.4 define the promise; this path would test evidence provenance rather than output shape.

### 54. Add failure injection at the seams users actually cross

**Approach.** Build opt-in deterministic failpoints around commit-before-response, outbox publish,
authorization refresh, KMS resolution, approval wake, budget settle, provider delivery, and process
termination. Drive them through a small set of black-box user journeys, and require recovery to be
visible and retry-safe rather than merely panic-free.

**Previous ledger.** `8d4a2827` (“Keep settlement outages from crashing agent workers”), `eed14f0c`
(“Keep approval storage outages fail closed”), `e5976170` (“Fail closed when Git pack state is
unavailable”), and review `07`’s operational-failure gap provide the failure catalogue.

## J. Client experience and operability

### 55. Extract one paginated collection model for the web app

**Approach.** Design a shared resource or primitive for initial loading, retry, empty state, keyset
pagination, refresh, optimistic insert, abort, and bounded error presentation. Migrate two unlike
pages first—such as Issues and repositories—to prove the abstraction describes behavior rather than
only matching markup, then remove duplicated state machines incrementally.

**Previous ledger.** Review `06-HIGH` identified six copies; `0f584749` (“Let the unified inbox load
every page”) and `8308027e` (“Fail closed on corrupt CI surface cursors”) provide real pagination
edge cases the abstraction must preserve.

### 56. Move approvals out of the application shell

**Approach.** Extract approvals into a focused component/domain module with typed decision outcomes,
loading and stale states, keyboard behavior, and shared error surfacing. Keep the shell responsible
only for layout and navigation, and exercise a failed decision plus successful retry in the real
browser.

**Previous ledger.** `95ad1827` (“Surface automation approvals in the shared inbox”), `419aa51f`
(“Make app recovery explicit and retryable”), and review `06-HIGH` explain how a useful feature
accumulated inside `AppShell` and where its missing failure handling belongs.

### 57. Grow the design system from repeated semantics

**Approach.** Add button, status, action-row, and list-state primitives only where repeated behavior
and accessibility are already understood. Replace the parallel status badges and common inline
layout styles, keep the token vocabulary open to domain states, and use visual/browser contracts to
guard focus, disabled, busy, destructive, and error behavior.

**Previous ledger.** The design-system cleanup entry `9d4968b3` and frontend review `06-MED/LOW`
identify repetition, while `301d99d2` (“Clarify frontend model commentary”) reinforces that the
abstraction should make code easier to read rather than merely reduce line count.

### 58. Converge the CLI on one parsing and dispatch shape

**Approach.** Compare the hand-rolled trailing-argument commands with the `clap::Subcommand` paths,
choose one model, and migrate by product noun with snapshot and black-box request tests. Preserve
excellent recovery messages, global profile/context precedence, stdin behavior, and idempotency keys;
reject silently ignored flags at compile- or parse-time.

**Previous ledger.** `7504f87d` (“Align CLI commands with product nouns”), `5d511011` (“Make issue
import resumption automatic”), review `06`’s dead `--resume` and dual-parser findings, and
`2dc629f0` (“Format CLI integration stories”) define both the strength and the cleanup target.

### 59. Test secret custody and session recovery as user stories

**Approach.** Exercise OS-keyring unavailable/locked/corrupt states, config-write failure, multiple
profiles for one Edge, expired session, browser denial, token-exchange response loss, Git helper use,
logout, and cleanup. Tests should assert observable commands and the absence of credentials in files,
process arguments, logs, protocol output, and error bodies.

**Previous ledger.** `328e838a` (“Test the CLI secret storage boundary”), `9988f566` (“Keep CLI
credentials in the OS keyring”), `b66b8ab0` (“Make CLI token exchange response-loss safe”), and
review `06-MED` call for turning indirect keyring coverage into a complete trust story.

### 60. Build an operator view around stuck work, not raw tables

**Approach.** Provide bounded queries for failed or aging workflows, firings, approvals, outbox rows,
consumer lag, CI leases, agent reservations, and credential teardown. Correlate them by canonical run
or operation identity, explain the safe recovery action, and make every repair command idempotent and
audited.

**Previous ledger.** `3ece507d` (“Keep hosted agent failures diagnosable”), `2a2eafa5` (“Show failed
automation guidance across clients”), `44573dc4` (“Make automation rule failures diagnosable”), and
contract 1.8 supply signals that currently lack one coherent operational narrative.

## K. Pleasant code and truthful tests

### 61. Decompose the next largest file around invariants

**Approach.** Re-rank production files by size, change frequency, security impact, and number of
independent responsibilities. Choose one high-risk file and extract typed boundaries with private
constructors before changing behavior; keep SQL near the repository that owns its transaction and
move test support out of production modules.

**Previous ledger.** `8c2bc706` (“Decompose the durable Git backend”), `3357beaf` (“Decompose
governed approval routing”), `fcfb0566` (“Consolidate durable scheduler claim fences”), and the Aug 10
review’s large-module criticism show the preferred refactoring style: converge semantics first.

### 62. Remove remaining production-shaped test doubles and dead exemptions

**Approach.** Audit public constructors, in-memory stores, scripted engines, `dead_code` allowances,
and feature-gated helpers in security-critical crates. For each, either prove a production caller,
move it under test support, replace it with a trait-bound fake at the test boundary, or delete it and
rewrite the claimed story against durable storage.

**Previous ledger.** `14b5c660` (“Remove in-memory scheduler twins”), `06fff602` (“Keep in-memory KMS
helpers test-only”), `13b2dc95` (“Keep MCP test doubles out of production”), and `38d025f7` (“Remove
stale sandbox dead code exemptions”) establish the cleanup doctrine.

### 63. Finish replacing source-text tests with structural or behavioral proof

**Approach.** Inventory `include_str!`, source substring counts, topology assertions, and opt-in marker
comments. Replace security boundaries with module privacy and typed construction, migration rules
with semantic audit, and runtime claims with executed tests; delete assertions that merely hash the
current implementation shape.

**Previous ledger.** `1be5c942` (“Retire source-text architecture lints”), `08789a77` (“Replace
sandbox source pins with behavior tests”), `c66ed3cb` (“Replace gVisor source pins with module
boundaries”), and `8263e592` (“Prefer behavioral coverage over source scans”) are the explicit
lineage for completing this sweep.

### 64. Make live-backend tests impossible to pass vacuously

**Approach.** Find integration tests that return early when Postgres, NATS, gVisor, or another required
backend is missing. Under the integration profile they should fail with a concise setup diagnosis;
tests that are legitimately optional should be explicitly ignored with a reason and a separate
release job that runs them.

**Previous ledger.** `a87937b` (“Make agent service integration tests fail loud”), `b3324b52`
(“Make hosted agent integration tests fail loud”), `f1370dab` (“Require live storage for agent
integration stories”), and review `07` name the anti-vacuity rule already adopted in part.

### 65. Give TypeScript system stories a small narrative vocabulary

**Approach.** Extract helpers named for user actions and observations—sign in, create a project, push
a branch, wait for a check, delegate a task, decide an approval, inspect linked work—while keeping
HTTP details available at assertion boundaries. Use unique fixtures and bounded polling so tests read
like durable product narratives without hiding response contracts or failure evidence.

**Previous ledger.** `07bcfc67` (“Add black-box backend system test harness”), `d488fa5e` (“Exercise
the complete Git lifecycle externally”), `4028a885` (“Exercise the assembled product edge”), and the
review `07` praise for real black-box coverage establish the style worth making easier to sustain.

### 66. Run a recurring vision-to-production seam audit

**Approach.** Periodically sample each README journey and high-fan-in contract from public entry point
to durable sink, then through failure and recovery. Record the evidence, stale claims, dead modules,
duplicated policy doors, and missing system stories; convert only confirmed gaps into work, and close
each exploration with a commit plus a user-readable proof.

**Previous ledger.** `e4156eeb` (“Remove the simulated Issues flagship scorecard”), `eed499b3`
(“Remove the self-fulfilling Git E2E wedge”), `85e29cdb` (“Enforce data boundaries without source
scans”), the Aug 10 review’s “new door skips the old guards” theme, and contracts 1.6 and 1.8 define
the continuous discipline that keeps all other entries honest.
