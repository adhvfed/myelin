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

Every person can keep a small stable of named agents that reflect how they work: their instructions,
tool palette, runtime, project context, and spending boundaries are explicit and editable. Starting
work with one creates a named cloud thread with a durable branch, conversation, and workspace. The
same workspace survives model context changes and can be opened through ordinary SSH or SFTP with a
short-lived owner-authorized credential.

An agent's owner can maintain private default workspace instructions on that named agent. The
organization-visible roster exposes only a revision and content digest; plaintext is encrypted as
owner-scoped agent data and available only through the owner's edit surface. Each new thread
snapshots the current defaults, then appends any thread-specific instructions as the more specific
layer. Editing the named agent affects future work without silently rewriting an existing thread,
including an idempotent retry of that thread's original creation request. Edits use an expected
revision, while an exact semantic retry converges on the already-written state.

The web product gives each person a direct roster for their own named agents; maintaining an agent
must not require beginning an unrelated thread. Organization agents owned by somebody else can
still be selected where policy permits, but their private configuration is never presented as an
editable action to the viewer.

The workspace is personalizable at creation time. Repository guidance remains the shared default,
while an owner can layer their own way of working into the assigned workspace—for example, custom
root `AGENTS.md` instructions—without changing the source repository. Personalization is durable
across fresh agent contexts and provisioning recovery, encrypted as owner-scoped agent data at
rest, bounded, and projected publicly only as an integrity receipt. Overlays must use confined
workspace writes and must fail closed on symlinks or conflicting durable markers. This initial
`AGENTS.md` seam should grow into a small declarative workspace augmentation model rather than an
unbounded startup-script escape hatch.

Exploration is private by default. A personal agent thread, its conversation, branch intent,
environment receipt, and workspace metadata are visible only to its owner and selected agent while
the work is a draft. Project membership alone must not reveal that the draft exists.

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
default journeys must prove plaintext is owner-only, stale edits conflict, new work receives the
latest revision, thread-specific instructions take precedence, and existing work retains its
original snapshot after the defaults change.
