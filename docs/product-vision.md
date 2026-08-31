# Product vision

Myelin is a pleasant, maintainable development platform where an organization can begin with one
login and one repository instead of assembling separate GitHub, Linear, Slack, CI, secrets, and
agent credentials. Humans and agents work through the same durable projects, conversations,
repositories, environments, permissions, and audit trail.

The product should feel useful before it feels configurable. A repository can declare its local
stack with `fed.yaml` and optional `.fed` configuration; Myelin can pin that declaration to an exact
source revision and use it as the default environment recipe. Projects that do not use Service
Federation remain first-class.

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

Hosted work is a product journey, not a CLI escape hatch. From the Automations surface, an owner can
choose one of their active hosted agents, name the platform event and bounded task, and set the
per-firing budget, lifetime firing limit, and human-approval rule. The safe initial form always
refuses personal-data events and delegates no extra caveats; advanced constraints may remain in the
CLI until the web can present them without obscuring their authority. Creation uses a retained
idempotency identity, lands on the durable rule, and exposes the same pause, resume, disable, firing,
approval, result, and erasure history as rules created by other clients.

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

This creates a humane gradient rather than a false choice between secret local work and noisy public
automation:

```text
personal draft -> owner and agent iterate -> owner marks ready -> project reviews shared work
```

Future slices may add named reviewers, reversible archival, merge/PR handoff, and richer agent
profiles, but they must preserve the private-by-default boundary and use the same production
authorization path as every other Myelin artifact.

## Evidence standard

Product claims are proved as readable TypeScript journeys against running services and durable
stores. A privacy journey must use two real identities and demonstrate both sides of the boundary:
the collaborator receives an indistinguishable not-found response before publication, then sees the
work after the owner's explicit transition, while workspace execution and SSH remain denied.
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
Web automation journeys must activate a real hosted identity, create a bounded rule through the
browser, reload its durable detail, verify budget, firing, approval, and personal-data constraints,
and disable it through the authenticated product boundary. Existing full-system event journeys must
separately prove that a matching durable event actually reaches approval and hosted execution.
