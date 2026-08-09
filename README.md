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
myelin context current
myelin auth configure-git

myelin chat list
myelin chat create engineering --topic "Release coordination" --idempotency-key release-room
myelin chat send 01J... "Ready for review." --idempotency-key release-message
myelin chat history 01J...

myelin kb page list
myelin kb page create --title "Deployment runbook" --template runbook \
  --idempotency-key deployment-runbook
myelin kb page get 01J...
```

Each named profile bundles its Edge, tenant, region, optional project, and an opaque credential
reference in `~/.config/myelin/config.toml`. The credential itself stays in the operating-system
credential store. `--profile` and `MYELIN_PROFILE` select a profile for one command; `context use`
changes the default without copying a token. Git helpers are scoped to an exact Edge and profile.

Mutation keys are the caller's single retry identity: reuse the same key after a lost response.
Chat and Knowledge derive their bounded durable nonce from it, so callers do not need a second
backend-specific token.

## Status

Pre-release and under heavy development. Interfaces, schemas, and deployment
shape change without notice.

## License

[FSL-1.1-ALv2](LICENSE.md) — free to use, modify, and self-host; you may not
offer it to others as a competing hosted service. Each release becomes
Apache-2.0 two years after publication.
