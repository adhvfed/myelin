# Myelin

A self-hostable software collaboration platform: git hosting, issues, CI, chat,
and agents in one platform, built in Rust.

## What's inside

- **Git hosting** — smart HTTP wire protocol, durable refs with transactional
  ref-CAS, push/pull backed by Postgres object storage.
- **Issues** — tracker with SLA timers and notifications; Myelin tracks its own
  issues on itself.
- **CI** — control plane, dispatch, and gVisor-sandboxed runners with
  secret-redaction and hardened credential handling.
- **Chat & agents** — chat gateway, agent runtime, and MCP integration.
- **Platform** — multi-tenancy, OIDC identity, search, knowledge, GDPR
  tooling, event outbox on NATS JetStream.

## Layout

- `crates/` — the Rust workspace (~38 crates, service binaries under
  `myelin-edge`, `myelin-control-plane`, and friends).
- `frontend/` — pnpm workspace with the SolidStart web app (`apps/web`).
- `deploy/` — systemd units.
- `scripts/` — development, build, and operational helpers.

Runtime dependencies: Postgres 16, NATS 2.10 (JetStream), Valkey 8, and an
S3-compatible object store.

The Git smart-HTTP service executes Git inside gVisor. Local development therefore
also expects a systemd user manager, `runsc` on `PATH`, and a Git-capable root filesystem at
`$XDG_DATA_HOME/gvisor-assets/git-rootfs` (or
`~/.local/share/gvisor-assets/git-rootfs`). Set `MYELIN_RUNSC_BIN` or
`MYELIN_GVISOR_GIT_ROOTFS` to override those locations. The self-hosting scripts
stage these assets; `scripts/stage-git-rootfs.sh` can restage the Git image.

## Development

Install [`fed`](https://www.service-federation.com/docs/), then start the complete local application:

```sh
fed start
```

Fed prints the allocated web and edge URLs when the stack is ready. Use `fed ports list`
to inspect the checkout's allocations at any time; ports are deliberately assigned by Fed
rather than fixed in application documentation.

The test entry points have separate responsibilities:

- `fed test:system` runs the standalone TypeScript black-box suite against the real Edge and
  durable services. It covers platform security, Git/PR/review/merge, Issues, Chat, Knowledge,
  CI event delivery, notification inbox contracts, retries, conflicts, and bounded errors.
- `fed test:integration` drives the assembled web application in a real browser.
- `fed test:backend` runs Rust tests that require the Fed-managed infrastructure.
- `fed test:frontend` runs frontend lint, type, unit, and build checks.
- `fed test:browser-contract` runs the faster browser suite against its contract backend.

Pass Cargo selectors after `--` to run a focused backend test, for example:

```sh
fed test:backend -- -p myelin-edge --test integration_issue_routes_pg
```

Pass Vitest selectors in the same way to focus the external suite:

```sh
fed test:system -- tests/git-lifecycle.system.test.ts
```

The `myelin` CLI mirrors the same Edge API used by the web application and agents. Alongside Git,
Issues, CI, and Notifications, its collaboration surface includes:

```sh
myelin --edge https://myelin.example auth login
myelin --profile customer-a --edge https://customer-a.example auth login
myelin context list
myelin context use customer-a
myelin context use --project 11111111-1111-1111-1111-111111111111
myelin context current
myelin auth configure-git

myelin tool list
myelin tool show ci.read_run
myelin agent create "Review companion" --tool ci.read_run --tool git.open_pr \
  --tool git.list_repositories --tool git.search_code --tool git.read_file \
  --tool issues.list --tool issues.view \
  --tool knowledge.list_pages --tool knowledge.read_page \
  --tool chat.list_conversations --tool chat.post --tool chat.read_messages \
  --idempotency-key review-companion
myelin mcp serve --as 22222222-2222-2222-2222-222222222222
myelin agent suspend 22222222-2222-2222-2222-222222222222 \
  --idempotency-key pause-review-companion
myelin agent resume 22222222-2222-2222-2222-222222222222 \
  --idempotency-key resume-review-companion
myelin agent retire 22222222-2222-2222-2222-222222222222 \
  --idempotency-key retire-review-companion

myelin agent create "Mainline triage" --runtime hosted \
  --tool ci.read_run --tool issues.create --idempotency-key mainline-triage
myelin automation create --event ci.run.failed --branch main \
  --run-as 33333333-3333-3333-3333-333333333333 \
  --task "Read the failed run and open one focused issue." \
  --budget-minor-units 250000 --max-firings 10 --require-human-approval \
  --idempotency-key red-mainline-triage
# Every event domain uses the same bounded query language for finer intent:
myelin automation create --event issue.issue.updated \
  --where "payload.change_kind == 'ownership'" \
  --run-as 33333333-3333-3333-3333-333333333333 \
  --task "Read the ownership change and suggest the next useful step." \
  --budget-minor-units 100000 --max-firings 10 \
  --idempotency-key issue-ownership-review
myelin automation list
myelin automation show 44444444-4444-4444-4444-444444444444
myelin automation history 44444444-4444-4444-4444-444444444444
# Read the completed run id from history, then inspect the agent's durable work product:
myelin automation result 44444444-4444-4444-4444-444444444444 \
  55555555-5555-4555-8555-555555555555
# Erase that result and leave a durable marker that prevents a retry from recreating it:
myelin automation erase-result 44444444-4444-4444-4444-444444444444 \
  55555555-5555-4555-8555-555555555555 --idempotency-key erase-triage-result
myelin automation approve 44444444-4444-4444-4444-444444444444 ci-failed-01J... \
  --idempotency-key approve-red-mainline
# Or end that exact pending firing without starting an agent run:
myelin automation reject 44444444-4444-4444-4444-444444444444 ci-failed-01K... \
  --idempotency-key reject-red-mainline
myelin automation pause 44444444-4444-4444-4444-444444444444 \
  --idempotency-key pause-red-mainline
myelin automation resume 44444444-4444-4444-4444-444444444444 \
  --idempotency-key resume-red-mainline
myelin automation disable 44444444-4444-4444-4444-444444444444 \
  --idempotency-key retire-red-mainline
# A hosted run that reaches a sensitive effect parks until this exact gate is decided:
myelin agent approve gate:0123456789abcdef0123456789abcdef \
  --idempotency-key approve-exact-merge
myelin agent reject gate:fedcba9876543210fedcba9876543210 \
  --idempotency-key reject-exact-merge

# Inspect the agent data held for the signed-in person, then erase that narrow scope:
myelin privacy agent-data status
myelin privacy agent-data erase --confirm

myelin repo list
myelin repo pr list
myelin project create "Developer experience" --prefix DX --idempotency-key developer-experience
myelin project show
myelin project list
myelin issue create "Make onboarding uneventful" --idempotency-key onboarding-issue
myelin issue list
myelin issue import --from jira --job 33333333-3333-3333-3333-333333333333 \
  --input jira-issues.json --dry-run
myelin issue import --from jira --job 33333333-3333-3333-3333-333333333333 \
  --input jira-issues.json --run --idempotency-key jira-import-3333
myelin inbox list

myelin chat list
myelin chat create engineering --topic "Release coordination" --idempotency-key release-room
myelin chat send 01J... "Ready for review." --idempotency-key release-message
myelin chat ref 01J... myelin://acme/issue/issue/ENG-41 --idempotency-key release-issue
myelin chat history 01J...

myelin doc page list
myelin doc page create --title "Deployment runbook" --template runbook \
  --idempotency-key deployment-runbook
myelin doc page get 01J...
```

Each named profile bundles its Edge, tenant, region, optional project, and an opaque credential
reference in `~/.config/myelin/config.toml`. The credential itself stays in the operating-system
credential store. `--profile` and `MYELIN_PROFILE` select a profile for one command; `context use`
changes the default without copying a token. `context use --project` records a project on the
active profile; `--project`, `MYELIN_PROJECT`, and that saved value override one another in that
order. Creating a project with a saved profile makes it active for that profile; its owned issue
prefix and default issue type then keep ordinary issue and conversation creation free of UUID and
prefix ceremony. Chat rooms follow that project's live collaborators, so private project rooms do
not appear to the rest of the tenant. The web Issues and Chat workspaces discover the same
authorized project catalogue. A new organization can create its first project inside the New issue
flow; established organizations choose a project by name and key. The browser never needs
deployment-provided project, type, or prefix identifiers.
Git helpers are scoped to an exact Edge and profile.

External MCP clients can use `myelin mcp serve --as <agent-id>` as their server command. The CLI
exchanges the saved browser-approved session for a one-minute run identity, keeps the bearer off
protocol output, and closes it when the client disconnects. Suspending or retiring the agent also
terminates every unfinished run atomically; resuming permits fresh work but never revives an old
run. No provider API key or long-lived agent credential is created or copied through this flow.
Selected read tools resolve CI runs, issues, and Knowledge pages through the human delegator's
live Myelin permissions, returning canonical references without GitHub, Linear, or Notion keys.

Hosted agents use the same identity and tool catalogue, but Myelin owns their execution. An
`automation` binds a canonical platform event to one hosted agent, a plain-language task, an
integer minor-unit budget, optional delegation caveats, and safety gates. Each firing receives a
short-lived run identity and reaches Git, CI, Issues, Chat, and Knowledge only through governed
Myelin tools. Repeating `--caveat` with capabilities such as `issue.create` narrows which selected
tools reach the model and the signed run identity; `--caveat repo:platform/api` additionally binds
every Git read and mutation to that repository. Invalid or merely decorative caveats are refused
when the automation is created. Owners can inspect durable firing history and outcomes, pause new
reservations for maintenance, resume them, or irreversibly disable an automation from either the
CLI or the web Automations workspace. Disable atomically closes every unfinished firing and its
workflow, releases unused run budget, rejects still-pending effects, and removes their approval
cards. Every run that reaches a final agent answer writes it before cost settlement as one
immutable, content-addressed Knowledge
trace. Its answer and block-model body rest only as authenticated ciphertext under the
requesting human's durable subject key. The automation owner can retrieve that work product and
its exact metered cost in the web firing history or with `automation result`; other users cannot
read it through the run reference. The owner can erase one result from that result view or with
`automation erase-result`; a durable erasure marker makes the operation idempotent and refuses any
later hosted-worker attempt to recreate the trace. A human-approval gate parks the exact
event without starting or paying for an agent run. It appears in the owner's shared inbox
with approve and reject actions in the web app and a copyable CLI command; either decision is
durable and retry-safe, completes the inbox item, and remains visible in automation history. No
third-party integration key is created or copied into the agent.

The durable trace store is also the H17 personal-data holder. A signed-in person can inspect this
narrow agent-data scope and irreversibly erase their own traces, model replay steps, and tool-effect
journals through Edge or `myelin privacy agent-data`. Erasure records a durable suppression marker,
deletes every live record, destroys the subject key so ciphertext in backups remains unrecoverable,
and permanently blocks later agent processing for that person. This self-service operation is not
presented as full account or organization erasure; other personal-data holders remain outside its
explicit scope.

Sensitive effects such as `git.merge` have a second, narrower approval boundary. The agent may
reason up to the effect, but Myelin withholds the mutation, parks the durable workflow without
holding a model runtime, and places the exact pull request in every eligible human approver's
shared inbox. Web and CLI decisions are durable and idempotent. Approval wakes the same workflow
with a fresh attempt-scoped run credential and can authorize only that effect once; rejection or
expiry settles the run without applying it and closes every approver's copy of the card. The agent
remains the attributed actor while its active human delegation supplies repository visibility, so
neither side needs a GitHub token.

Three-segment events carry their subject type in the name, so `issue.issue.updated`,
`knowledge.page.updated`, and `chat.message.created` need no extra matcher ceremony. Ambiguous
two-segment events such as `ci.result` use `--subject-type run`. Myelin currently admits subjects
whose live visibility can be checked end to end: CI runs; Git repositories and references; Git
pull requests and comments; Issues issues; Knowledge pages and rows; and Chat
channels and messages. Git-reference visibility follows the containing repository, so a branch
automation cannot reveal an update from a repository its owner cannot pull. Other artifact
types are refused at creation instead of becoming automations that can never fire.

`--where` adds a bounded `myelin-query` predicate to the exact event match. It can compare scalar
envelope fields such as `event.depth` and scalar payload fields such as `payload.change_kind`,
using `==`, `!=`, `<`, `<=`, `>`, `>=`, `AND`, `OR`, `NOT`, and parentheses. A missing or
wrongly-typed field closes only that rule as a non-match; it cannot prevent another automation
from seeing the event. The automation retains the newest evaluation error for its owner in the
web/API and CLI output, including the event and time that exposed it. `--branch main` remains
concise sugar for
`payload.source_ref == 'refs/heads/main'`. CI runs keep that scope consistent: it is the pushed
branch for push runs and the target branch for pull-request runs.

Issue responses include a canonical `myelin://...` reference that can be passed directly to
`chat ref`. The conversation keeps a live pointer to the issue rather than copying a stale
snapshot.

Mutation keys are the caller's single retry identity: reuse the same key after a lost response.
Chat and Knowledge derive their bounded durable nonce from it, so callers do not need a second
backend-specific token.

Issue import input is a strict JSON object containing up to 256 records. The active project is
applied to every record, so project scope stays in the selected context rather than being repeated
throughout an export:

```json
{
  "records": [
    {
      "source_id": "JIRA-41",
      "type_id": "22222222-2222-2222-2222-222222222222",
      "prefix": "ENG",
      "title": "Make onboarding uneventful"
    }
  ]
}
```

`--dry-run` performs validation and authorization checks without writing. Every `--run` is
resumable: reuse the same `--job` after an interruption and the durable source-ID map returns
existing issues instead of creating duplicates. Use `--input -` to read the document from standard
input.

## Status

Pre-release and under heavy development. Interfaces, schemas, and deployment
shape change without notice.

## License

[FSL-1.1-ALv2](LICENSE.md) — free to use, modify, and self-host; you may not
offer it to others as a competing hosted service. Each release becomes
Apache-2.0 two years after publication.
