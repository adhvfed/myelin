# Product vision

Myelin is a pleasant, maintainable development platform where an organization can begin with one
login and one repository instead of assembling separate GitHub, Linear, Slack, CI, secrets, and
agent credentials. Humans and agents work through the same durable projects, conversations,
repositories, environments, permissions, and audit trail.

The product should feel useful before it feels configurable. A repository can declare its local
stack with `fed.yaml` and optional `.fed` configuration; Myelin can pin that declaration to an exact
source revision and use it as the default environment recipe. A malformed or unrelated root file
retains its pinned digest but is visibly invalid rather than silently disappearing or being blessed
as a declaration. Projects that do not use Service Federation remain first-class.

Everyday work should remain forgiving. A person can create, find, close, and reopen an issue through
the web or CLI under the same project-derived transition authority. Close and reopen are durable,
retry-safe state changes with metadata-only audit events; reopening restores the issue to Backlog
without discarding its identity or history. Richer configurable workflows must extend that durable
transition boundary rather than replacing it with client-side state.

Project ownership must be useful without an operator editing policy behind the product. A project
writer can discover the organization's other active humans by their profile names and give or
remove direct reader access from a dedicated project surface. The live ReBAC graph remains the only
membership source: direct readers immediately become project collaborators across Issues, Chat,
and agent review, while writer and team-inherited access is shown honestly and cannot be removed by
deleting an unrelated direct-reader edge. Directory reads are tenant-scoped, bounded, paged, and
available only to project writers; readers receive the same not-found boundary as outsiders.

## Personal cloud agents

Every person can keep a small stable of named agents that reflect how they work. Instructions and
the tool palette are editable; project context is an explicit choice on each piece of work; hosted
automation budgets and firing limits are explicit per use. Runtime is an explicit identity choice:
external MCP agents resume private cloud threads, while hosted agents run governed event-driven
automations without a provider credential. Runtime stays fixed after activation so existing threads
and automations cannot silently move execution environments; choosing another runtime creates a
distinct named identity. Starting private work creates a named cloud thread with a durable branch,
conversation, and workspace. The same workspace survives model context changes and can be opened
through ordinary SSH or SFTP with a short-lived owner-authorized credential. A person can also
authorize an existing Ed25519 key and generate an expiring, host-key-pinned OpenSSH profile for an
SFTP-capable editor; Myelin never copies or prints the private key, and refreshing access does not
replace the retained workspace.

A useful cloud branch is not a one-shot trial. While a workspace is live, its owner can explicitly
keep it for 7, 14, or 30 days from today. Renewal advances the branch, conversation retention
receipt, and owner/agent Chat authority to one exact deadline without replacing the workspace.
The deadline change is transactionally ordered before expiry cleanup and keyed by the client's
stable intent, so an ambiguous retry cannot compound the lifetime. Cross-store Chat repair has a
durable pending record and remains fail-closed until recovery completes. Existing agent-run and SSH
credentials keep their original short expiry; continuing work always obtains fresh authority
bounded by the renewed workspace.

Hosted work is a product journey, not a CLI escape hatch. From the Automations surface, an owner can
choose one of their active hosted agents, name the platform event and bounded task, and set the
per-firing budget, lifetime firing limit, and human-approval rule. The safe initial form always
refuses personal-data events and delegates no extra caveats; advanced constraints may remain in the
CLI until the web can present them without obscuring their authority. Creation uses a retained
idempotency identity, lands on the durable rule, and exposes the same pause, resume, disable, firing,
approval, result, and erasure history as rules created by other clients. New firings appear without
a manual reload; the owner can approve or explicitly confirm rejection in place, follow subsequent
progress, and reload the page without losing either decision. Agent selection traverses the owner's
complete paged catalogue, including the case where no eligible hosted identity appears in the
newest page; an unavailable catalogue is presented as a retriable failure, never as proof that the
owner has no agent. When no hosted identity exists, the activation handoff preserves that intent and
opens with the hosted runtime selected; ordinary activation elsewhere continues to default to an
external private-work identity.

An agent's owner can maintain a private default workspace augmentation on that named agent:
`AGENTS.md` instructions plus bounded create-only context files. The organization-visible roster
exposes only a revision and content digest; plaintext is encrypted as owner-scoped agent data and
available only through the owner's edit surface. Each new thread snapshots the current defaults,
then appends any thread-specific instructions and files as the more specific layer. Editing the
named agent affects future work without silently rewriting an existing thread, including an
idempotent retry of that thread's original creation request. Edits use an expected revision, while
an exact semantic retry converges on the already-written state. Instruction-only clients preserve
configured files they do not know how to edit.

The web product gives each person a direct roster for their own named agents; maintaining an agent
must not require beginning an unrelated thread. Organization agents owned by somebody else can
still be selected where policy permits, but their private configuration is never presented as an
editable action to the viewer. An owner can rename a non-retired agent without replacing its
identity, retained threads, workspaces, policy, tool palette, or live authority. Names remain unique
within the organization; optimistic replacement detects a genuinely stale view while an exact retry
converges on the saved name. Audit evidence records only the stable agent identifier because a
human-chosen name can itself contain personal data. The same owner surface can pause an identity
immediately, resume it later, or retire it permanently with an explicit confirmation; pausing or
retirement revokes live agent authority without making retained work disappear.

The tool palette is equally concrete. Activation shows the complete live platform catalogue rather
than hiding a fixed authority bundle behind a broad toggle, and the personal roster lets the owner
replace the exact selection later. A palette update re-derives grants from the catalogue under the
owner's current authority ceiling, advances the agent and delegation policy heads, updates the
registration, terminates unfinished runs, and emits metadata-only audit evidence in one durable
transaction. Run creation binds its claim to the registration's exact tool-and-grant snapshot, so
a concurrent update cannot mint stale authority. Optimistic edits compare semantic tool names so a
catalogue version advance does not strand an older agent; exact retries converge, while a genuinely
stale selection conflicts. Existing workspace data remains intact and only a fresh connection can
receive the new palette.

CI is part of that governed working surface, not a read-only dashboard or another provider token.
An agent can request a retry of one exact failed, timed-out, cancelled, or reaped run when its human
delegator still has push authority on the source repository. The retry always requires explicit
human approval, preserves the immutable repository, branch, commit, definition snapshot, trust
tier, and causal provenance, and allocates a fresh run, workflow, and check attempts. Approval cards
inherit visibility from the run's live source repository rather than a synthetic CI permission.
Exact retries converge on the same requested run and event; they never buy or launch duplicate
compute. The same agent can ask to cancel one exact queued or running run under the same approval
rule. People with push access do the same directly: the run page, `myelin ci retry|cancel`, and the
API re-run a finished run or stop an active one, with a confirmation before anything is interrupted.
A human re-run is recorded as `manual` and keeps the source branch; an exact retry of the same intent
returns the same run. Cancelling, whether by an agent or by supersession, stops the sandbox within seconds rather
than letting abandoned compute run to its timeout, and its measured usage settles exactly once.

The workspace is personalizable at creation time. Repository guidance remains the shared default,
while an owner can layer their own way of working into the assigned workspace—for example, custom
root `AGENTS.md` instructions and private context files—without changing the source repository.
Personalization is durable across fresh agent contexts and provisioning recovery, encrypted as
owner-scoped agent data at rest, bounded, and projected publicly only as an integrity receipt.
Declarative files are create-only: an identical existing file makes recovery idempotent, while a
different file, symlink, traversal, `.git` target, or attempt to replace the dedicated `AGENTS.md`
layer fails closed. The initial model deliberately permits at most eight files and 12 KiB of total
encrypted augmentation data. Richer strategies can grow from this explicit contract; arbitrary
startup scripts must not become a configuration escape hatch.

Exploration is private by default. A personal agent thread, its conversation, branch intent,
environment receipt, and workspace metadata are visible only to its owner and selected agent while
the work is a draft. Project membership alone must not reveal that the draft exists. New work starts
personal-only; a future review project is attached only through an explicit choice, never by silently
selecting the first project the viewer can see.

The owner can deliberately mark project-bound work **ready for review**. That one-way transition
makes the durable conversation visible to the project's current collaborators and clearly records
who shared it and when. It does not grant collaborators agent-run, command, SSH, or infrastructure
authority; those capabilities remain owner-controlled. Publishing a review surface must never
publish reusable credentials, host paths, model context, or unrelated workspace data.

Ready code has a deliberate second boundary. The owner can publish one committed descendant of the
pinned workspace base into its durable Myelin repository and open a pull request. Until that action,
the workspace branch does not exist in the shared repository. Publication refuses ordinary
uncommitted changes, divergent history, and any commit that contains the owner-only `AGENTS.md`
layer, private context paths, or Service Federation's generated `.fed` runtime state. The committed
`.fed/cloud.yaml` project link and `.fed/.gitignore` remain reviewable source configuration; lock
databases, generated or cached secrets, logs, a file or symlink replacing the reserved `.fed`
directory itself, and future runtime artifacts never cross the handoff.
A clean tip is not enough: before moving a durable ref, Myelin scans every base-exclusive commit and
introduced blob with the same bounded object-size and credential-content policy as an ordinary Git
push. Deleting a credential in a later commit therefore does not make the reachable review history
publishable; the owner must rewrite it out of the branch. History and aggregate object limits keep
this pre-publication inspection bounded. Commits, blobs, and changed Git tree metadata all cross the
same individual-object policy, while a separate delta ceiling charges deletion-only work that does
not introduce an object. High-confidence provider signatures cover the GitHub,
GitLab, and Slack token families that teams commonly remove from their collaboration plumbing;
minimum length and character-diversity checks keep prefix documentation and placeholders reviewable.
A stable thread-derived Git operation makes an interrupted retry
converge on one pull request. The workspace pane derives its reviewer choices from the project's
current authorization graph, presents only other active humans by their profile display name, and
lets the owner grant selected people access to that exact review without silently broadening
repository membership. The durable handoff survives reload and links the thread to the exact head
ref, commit, and pull-request number. This boundary is available from both the workspace pane and
`myelin agent thread pull-request`: remote-editor work should not force a developer to leave the
CLI or manually reconstruct an internal HTTP request. Both clients name the same stable intent and
render the existing pull request honestly on replay.
When publication is refused, the visible workspace pane preserves Edge's bounded safe explanation:
it distinguishes dirty work, private paths, likely credentials, oversized history, and divergence,
and names only the path the owner can act on. A generic conflict must not turn a safe refusal into an
opaque dead end, while storage and transport failures must remain unprojected.

The handoff remains useful while review is active. The owner can commit more work in the retained
workspace and deliberately update the same pull request from either the workspace pane or CLI.
Updates are fast-forward-only: the durable branch tip, pull-request head, and thread receipt advance
through one retry-safe saga with compare-and-swap checks at each boundary. Reviewer assignments
remain, but approvals, code-owner satisfaction, endorsements, and check facts for the previous head
are cleared. The chosen reviewers' durable inbox request is reopened on that same pull request, so
invalidating a decision cannot silently leave the human unaware that new work needs review. An event
with the new head generation lets CI and other consumers evaluate the exact commit now under review.
Each caller-supplied idempotency key is bound to one exact head transition:
replaying it is inert, while reusing it for later workspace work conflicts and requires a fresh
intent. Replaying an already-current update reports that state honestly rather than opening a
second pull request.

This creates a humane gradient rather than a false choice between secret local work and noisy public
automation:

```text
personal draft -> owner and agent iterate -> owner marks ready -> publish committed branch -> review
```

Future slices may add reversible archival and richer agent profiles, but they must preserve the
private-by-default boundary and use the same production authorization path as every other Myelin
artifact.

## Evidence standard

Product claims are proved as readable TypeScript journeys against running services and durable
stores. A privacy journey must use two real identities and demonstrate both sides of the boundary:
the collaborator receives an indistinguishable not-found response before publication, then sees the
work after the owner's explicit transition, while workspace execution and SSH remain denied.
Project-access journeys must begin with a second identity outside a newly created project, discover
that human through the writer-only product directory, grant access through the live authorization
graph, prove the reader can collaborate but cannot administer access, and revoke the direct grant
again. A browser journey must repeat the grant and revoke through the visible project surface.
Cloud-branch handoff journeys must additionally prove the ref is absent before the explicit action,
publish a real commit made inside the confined workspace, give a named reviewer access to the exact
pull request, survive a reload and retry, and demonstrate that private augmentation paths are absent
from the shared repository. The journey must force-add realistic Service Federation runtime state,
delete it in a later commit, prove the clean-looking tip is still refused without creating or
advancing the durable branch, and rewrite both commits inside the confined workspace. It must then
commit a realistically shaped provider credential, delete it in a later commit, prove that
clean-looking tip is also refused, rewrite that reachable history, and only then publish
successfully. It must then make
another real workspace commit, update the existing
pull request without changing its identity, and prove an exact replay is inert. The browser journey
must load reviewer choices from the live project authorization graph, select the named human through
the visible product, attempt a hidden provider credential, render Edge's exact safe rewrite
instruction, rewrite it through governed work, update the same review through its visible control,
and prove their exact PR access with a separate identity. The durable-store journey must also observe the reviewer request
arrive unread, complete after a submitted decision, and reopen on the same PR when a new head makes
that decision stale. The CLI journey must cross the actual device-login, MCP, SSH/SFTP,
editor-profile, Git commit, ready, and pull-request boundaries rather than certifying only argument
parsing or a mocked transport.
Workspace-personalization journeys must read the resulting file through an actual bound agent run,
open it again from a fresh context, and prove the repository source was never modified. Named-agent
default journeys must prove plaintext is owner-only, stale edits conflict, instruction-only edits
preserve configured files, new work receives the latest revision, thread-specific instructions take
precedence, and existing work retains its original instruction-and-file snapshot after the defaults
change. Tool-palette journeys must prove a second identity cannot edit, a real live run is
terminated, its credential stops authenticating, an exact retry is inert, a stale edit conflicts,
and a fresh run exposes only the saved tools while retaining its workspace.
Rename journeys must prove a second identity receives not-found, the owner sees occupied and stale
conflicts, an exact retry is inert, list and detail reads expose the saved name, and a live agent run
continues against the same retained workspace and authority.
Editor-profile journeys must generate a real key, create the profile through the product CLI, and
then leave Myelin's process: ordinary OpenSSH must use that profile for both a command and an SFTP
transfer against the same retained workspace. The proof must check file permissions, exact host
pinning, connection sharing, and absence of credentials from CLI output and generated config.
Workspace-renewal journeys must advance a live short-retention branch through the product, reload
the exact new deadline and retention receipt from durable stores, and continue private messaging
and workspace access without replacing its identity. Storage proof must race the old deadline,
replay the same intent, recover an interrupted Chat-authority update, and still expire the workspace
at the new exact boundary.
CI-effect journeys must create and execute a real failing pipeline, prove an unrelated human cannot
approve the retry, approve it as the repository owner, and observe a fresh terminal run in durable
storage with the same repository, branch, commit, definition snapshot, and trust tier. They must
also prove the retry event is co-committed exactly once and an exact MCP replay does not create a
third run or another event. Cancellation journeys must cancel a workload that is genuinely running
in gVisor and observe the run cancelled, its job cancelled, and its cost settled well before the
command would have exited.
Web automation journeys must activate a real hosted identity, create a bounded rule through the
browser, produce real durable platform events, observe incoming firings without a manual reload,
approve one, confirm rejection of another, reload both decisions, verify budget and personal-data
constraints, and disable the rule through the authenticated product boundary. Full-system event
journeys must additionally prove that an approved firing reaches hosted execution and its durable
result.
