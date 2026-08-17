# Myelin backend system tests

This package treats Myelin as an external system. It sends HTTP requests to the running
edge and observes durable, cross-service outcomes; it does not import application crates
or frontend modules.

Run it through Service Federation so every URL, credential, port, and dependency belongs
to the current checkout:

```sh
fed test:system
```

Fed supplies the ports and credentials allocated to the current checkout, waits for dependency
health checks, and applies its borrow-or-own lifecycle: an existing development stack stays up,
while services started for the command are stopped on exit. The suite uses a dedicated tenant and
unique resource names so repeated runs do not depend on pre-existing product data. Pass Vitest
selectors after `--`, for example:

```sh
fed test:system -- tests/platform.system.test.ts
```

`pnpm --dir system-tests typecheck` is service-independent. Running `pnpm test` directly
is intentionally unsupported because it would bypass Fed's allocated environment.

## Scope

The suite is intentionally organized around externally visible contracts:

- `platform` verifies health, readiness, capability authentication, and bounded routing errors.
- `git-lifecycle` follows repository creation through browsing, delivery of a real review request
  to a second principal, reviewer-scoped PR access, review and inbox completion, merge, and
  base-branch readback.
- `git-wire` uses a stock Git client to verify smart-HTTP authentication, repository grants,
  namespaced clone URLs, clone, fetch, and push.
- `search-lifecycle` verifies exact code coordinates, repository authorization, replacement of stale
  matches, default-branch isolation, promotion, and deletion through a stock Git push.
- `realtime-lifecycle` subscribes as an external tenant client and verifies authenticated repository
  creation and push events over the server-sent event stream.
- `collaboration-lifecycle` creates and safely retries a public Chat conversation, sends and pages
  messages through one public retry identity, and proves private project conversations cannot leak
  through backlinks on shared work. It creates and edits a Knowledge page with the same retry
  convention, exercises the asynchronous Issues authorization reconciler and optimistic conflicts,
  starts a project and its first issue without operator-provided scope IDs, and follows red mainline
  CI through a durable, retry-safe human approval into one
  governed hosted-agent run and one attributed issue. It also proves a visible Issues event uses
  the Issues permission boundary—not a hidden CI-only matcher assumption—before reaching its gate.
- `authentication-lifecycle` starts logged out, lets an authenticated browser identity approve one
  verifier-bound CLI login, proves another CLI cannot claim it, and uses the fresh session exactly
  once without copying the browser credential.
- `cli-authentication` approves two browser identities into named local contexts, switches between
  them without exposing either secret, configures and operates external and hosted agents plus
  event-driven automations without integration keys, lets an agent discover a visible project's
  canonical reference and create work from it without an operator-supplied database ID, follows
  listed issues and Knowledge pages by their canonical references, and binds native Git to exactly
  one profile,
  carries an agent issue closure through a canonical human approval card and an exact lost-response
  retry, refuses an expired session before transport, and removes both OS-backed credentials on
  logout.
- `automation-delegation` proves automation caveats are enforceable authority and that a
  short-lived agent can use its founder's repository visibility only through an audited MCP read,
  never by impersonating the founder at an ordinary product endpoint.
- `ci-lifecycle` proves that a Git push crosses the outbox/NATS/dispatcher boundary exactly once,
  executes the pushed commit in the pinned gVisor sandbox, settles successfully, and preserves the
  exact archived output while repository visibility still governs both list and detail reads.
- `notification-lifecycle` publishes through the external JetStream boundary and verifies durable,
  recipient-scoped delivery, addressable inbox items, broker de-duplication, collapse, read state,
  self-suppression, and complete duplicate-free traversal through opaque inbox cursors.
- `api-contracts` checks strict inputs, resource identifiers, payload limits, notification inbox
  scoping, and public error envelopes.

Tests generate unique resources inside the checkout-specific integration tenant. They may run
against an already-running stack or let Fed own the required services; either path uses the same
resolved ports and readiness gates.
