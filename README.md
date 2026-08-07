# Myelin

A self-hostable software collaboration platform: git hosting, issues, CI, chat,
and agents on one substrate, built in Rust.

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
- `scripts/` — build, drill, and dev-stack helpers.

Runtime dependencies: Postgres 16, NATS 2.10 (JetStream), Valkey 8, and an
S3-compatible object store.

## Status

Pre-release and under heavy development. Interfaces, schemas, and deployment
shape change without notice.

## License

[FSL-1.1-ALv2](LICENSE.md) — free to use, modify, and self-host; you may not
offer it to others as a competing hosted service. Each release becomes
Apache-2.0 two years after publication.
