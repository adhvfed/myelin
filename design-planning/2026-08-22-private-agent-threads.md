# Private agent threads and durable workspaces

Status: implementation design, 2026-08-22.

## The user story

A person starts a named, private thread with one of their agents and describes a problem. The
conversation never appears to people outside that thread. The agent works in a bounded workspace
which survives individual model runs, process restarts, and fresh model context. The person can see
the workspace's state and open an ordinary SSH session into it without configuring a permanent key
or copying an infrastructure credential. After a visible, bounded retention period, the workspace
becomes inaccessible and is deleted.

The intended CLI reads as one journey:

```text
myelin agent thread start "Investigate checkout race" --agent <agent> --retention-days 3
myelin agent thread say <thread> "Reproduce the flaky checkout and prepare the smallest fix."
myelin agent thread show <thread>
myelin agent thread ssh <thread>
```

The web application should consume the same Edge resources. There is no privileged GUI-only or
CLI-only route.

## Product model

An `AgentThread` is the durable aggregate presented to a user. It owns:

- one `channel_private` Chat conversation;
- one selected agent identity;
- one human owner;
- an optional project context;
- one durable workspace generation; and
- a fixed `expires_at`, derived from a bounded retention choice.

The thread, conversation, and workspace have different identifiers because they have different
lifecycle and authorization concerns. Responses include canonical references between them; clients
must never infer one identifier from another.

An agent *run* does not own a workspace. A short-lived run leases the thread's current workspace and
is bound to the thread, conversation, and workspace generation. Closing or expiring a run revokes
its authority without deleting files. Starting work from fresh context issues a new run identity
against the same live workspace generation.

The first supported retention range is 1 through 30 whole days, with 3 days as the default. Creation
fixes `expires_at`; ordinary reads, messages, SSH sessions, and agent runs do not silently extend it.
Renewal will be an explicit, audited operation with the same upper bound. The API returns both the
chosen policy and the effective timestamp. This follows exploration 46: retention is observable
product behavior, not an invisible cleanup setting.

## Authority and privacy

The private conversation uses Chat's existing `channel.member` relationship. Creation writes direct
membership for exactly the owner and the selected agent. It does not write the
`project:<id>#view` membership used by public project rooms. Project context may narrow tools and
provide navigation, but project membership never grants thread visibility.

Every surface resolves through that same live relationship:

- thread list and lookup;
- message list, post, mention, live events, search, backlinks, and notifications;
- agent run creation and governed Chat/workspace tools;
- workspace state and SSH access; and
- later invitation, removal, archive, renewal, and erasure operations.

An unauthorized caller receives `not_found` for the thread, conversation, workspace, and SSH grant.
No list item, error distinction, event, notification, reference projection, hostname, or timing
metadata may disclose that the thread exists. This applies to tenant peers as well as other agents.

The owner must own the selected agent for the initial product floor. Agent suspension prevents new
runs and new SSH grants; retirement is absorbing. An already-issued run still obeys the existing
short-lived credential and live-delegation checks. Thread authority never broadens the agent's tool
catalogue or the owner's resource authority.

## Workspace lifecycle

The durable state machine is monotonic:

```text
provisioning -> ready -> expiring -> deleted
       |          |         |
       +------> failed <-----+
```

`failed` records a bounded, non-sensitive reason and a retryable operator action. A failure after
database creation cannot make a partially provisioned workspace accessible. Provisioning and
deletion use durable operation identities so process death can be reconciled. Cleanup first makes
all new access fail closed, then revokes active grants and run leases, then deletes storage, and only
then records `deleted`. A deletion failure remains inaccessible and visible to operators until it is
reconciled.

The workspace storage layer owns quota, filesystem identity, isolation, and deletion. The existing
CI `ManagedWorkspace` is useful evidence for these mechanisms but cannot be reused as the durable
owner: its RAII contract intentionally poisons admission when a workspace outlives its job. A new
durable workspace owner should reuse the storage invariants while making ownership explicit in the
database and recoverable at process boot.

Neither Edge nor a client receives a host filesystem path. Agent workspace tools resolve the
workspace from the signed thread-bound run context. Initial tools should be deliberately small:
bounded directory listing, bounded file read/write, and command execution through the existing
agent sandbox. Paths are workspace-relative, symlink-safe, and subject to output, time, process,
network, and disk limits.

## SSH without key configuration

`myelin agent thread ssh` generates an ephemeral Ed25519 keypair locally, requests a short-lived
workspace access grant using only the public key, invokes the user's installed OpenSSH client, and
deletes the private key when the client exits. The private key is never sent to Myelin or placed in
the profile. The grant stores only the public-key fingerprint and is bound to:

- tenant, region, owner, thread, workspace id, and workspace generation;
- an SSH-only audience and purpose;
- a maximum five-minute admission window; and
- an expiry no later than the workspace expiry or browser-approved session expiry.

The response contains the workspace gateway host, port, opaque SSH username, expiry, and pinned
host-key material. It contains no bearer token, filesystem path, cloud instance identity, or reusable
infrastructure secret. The workspace gateway authenticates the presented key against the live grant,
rechecks thread membership and workspace state, records an audit event, and enters the same isolated
workspace used by agent tools. Existing sessions receive a bounded grace period at expiry and are
then terminated.

This is a workspace protocol, not the Git clone protocol. It must not weaken or overload the
existing SSH human-authentication verifier.

## HTTP resources

The initial Edge surface is intentionally resource-shaped:

- `POST /v1/agent-threads` creates or replays a named thread and workspace request.
- `GET /v1/agent-threads` lists only live visible threads.
- `GET /v1/agent-threads/{thread}` returns conversation and workspace state.
- `POST /v1/agent-threads/{thread}/runs` issues a short-lived, thread-bound agent run.
- `POST /v1/agent-threads/{thread}/ssh-access` issues an ephemeral SSH grant.

Chat messages continue through `/v1/chat/conversations/{conversation}/messages`; the aggregate does
not invent a second messaging implementation. Every mutation uses the normal `Idempotency-Key`
contract. A repeated key returns the original resource only when the full intent matches.

Thread JSON includes `name`, agent and conversation references, workspace id and generation,
`state`, `retention_days`, `created_at`, and `expires_at`. It never returns internal locators or
credentials. Expired and deleted threads remain addressable to their owner as lifecycle receipts,
while their Chat, run, workspace-tool, and SSH surfaces refuse access.

## System story

The black-box TypeScript story should read as user behavior and prove the assembled system:

1. Alice activates an agent and starts a named private thread with three-day retention.
2. A retry returns the same thread, conversation, and workspace.
3. Bob cannot list, look up, read, post, subscribe, inspect the workspace, or request SSH access.
4. Alice posts the problem. A thread-bound run reads it and replies as the selected agent.
5. The agent writes a chosen marker into the workspace and closes its run.
6. A fresh run has a new credential but the same workspace generation and reads the marker.
7. Alice obtains an ephemeral SSH grant and an OpenSSH command reads the same marker.
8. The returned expiry is exactly the requested bounded policy and no access operation extends it.
9. Expiry makes messages, new runs, workspace tools, and SSH admission fail closed; cleanup deletes
   storage while preserving an owner-visible lifecycle receipt.
10. No response, CLI output, event frame, or test diagnostic contains a run token or private key.

The time-dependent part should use a production operator reconciliation command with an injected
clock in the Fed stack, not a test-only HTTP backdoor and not a multi-day sleep. Live Postgres tests
will also exercise crash points around provisioning, activation, expiry, grant revocation, and
deletion.

## Implementation sequence

1. Generalize the durable Chat path from public-only queries to relationship-visible conversations,
   then add direct-member private creation. Keep public project behavior unchanged.
2. Add the durable `AgentThread` and workspace-generation state machine with idempotent creation,
   owner/agent validation, retention bounds, and reconciliation tests.
3. Bind external-agent runs to a thread and add governed workspace tools. Prove continuity across
   two runs before exposing remote shell access.
4. Add the workspace gateway and ephemeral SSH grant exchange, then exercise it with the real
   OpenSSH client in the TypeScript suite.
5. Add expiry reconciliation, deletion evidence, and secondary-surface privacy checks. Only then add
   the GUI projection.

Each step should land as a small compiling commit with focused live-storage tests. The final story
runs through `fed test:system` after every changed service has restarted.

## Relationship to the existing ledger

This design is the concrete next slice of exploration 37: private Chat authority comes from explicit
membership and must hold across every secondary surface. It adopts exploration 12's typed,
purpose-bound treatment of SSH and agent-run credentials. The split between a durable thread and
short-lived runs extends `22475513` and `9b6059c2`, which made run closure explicit rather than an
object destructor side effect. Visible, fixed expiry follows exploration 46 and contract 10.5.

It preserves contract 4.1's single machine-identity vocabulary, contract 4.6's stamped relationship
writes, contract 4.9's Chat namespace, contract 8.1's governed tool catalogue, and contract 9.4's
multi-day durable work. It also answers the 2026-08-10 review's Chat finding without reviving its
failure mode: private threads use the already-live ReBAC path repaired by `8a494693`, never a handler
local `kind == private` permission check.

