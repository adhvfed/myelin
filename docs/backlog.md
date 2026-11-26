# Backlog

These are findings that don't block landing: low severity, pre-existing, or hardening. Each item names where it came from. Remove an item when it's done.

## Authorization

- **Deep exclusion chains fail open (pre-existing).**
  - Where: `check_engine.rs` `has_relation` returns false at `MAX_REWRITE_DEPTH`; `namespace.rs` does the same at `MAX_RULE_DEPTH`.
  - Effect: inside an exclusion's subtracted arm, that false reads as "not excluded", so a `confidential` chain longer than about 16 hops allows access.
  - Fix: make truncation an error that denies.
- **A depth refusal reads as an outage, not a deny.**
  - Where: `check_many`, `check_each`, `list_objects` and `list_subjects` propagate `Unavailable` when the read is refused for depth.
  - Effect: `repo_authz_live`'s fail-static path may then serve a stale cached Allow.
  - Fix: a deterministic refusal must be a Deny.
- **Migration 0195 lock risk.** It takes two ACCESS EXCLUSIVE ALTERs in one transaction, so it can deadlock with writers that read `rebac_object_revision` and then insert into `rebac_tuple`. It has no `lock_timeout`. It is applied, so the fix is a new migration plus a timeout for future DDL.
- **The 0196 boot refusal message isn't actionable.** It doesn't name the table, tenant, relation or subject, or give a query to find the row. One bad row stops every service.
- **0196 records nothing about what it changed.** It logs no counts of rows moved or merged per tenant, and it doesn't bump `rebac_revision` for affected tenants.
- **The tuple-written event now carries keyed objects.** Its consumers (`reverse_index` etc.) haven't been checked for reliance on the old spelling.
- **The over-deep refusal fires one hop early** when a userset at distance 31 names a further object. This fails closed.

## Notifications

- **No inbox error is rate-limited:** a log line per undecodable PR on every inbox request.
- **An empty page with HTTP 200 on a permission-snapshot failure.** The client can't tell it from "nothing readable".
- **A subject-filtered read with `limit=1` reveals a count** of notifications about a subject the viewer can no longer read, via `next_cursor`. The viewer is the recipient, so this is minor.
- **`cli-authentication.system.test.ts` (~2362) finds an approval item on page one of `view=all`.** It will break as the shared inbox grows; use the subject filter.

## Workers

- **Other worker drive errors still exit the hosted-agent worker** (e.g. a transient DB error whose lease release succeeds). Only a lost lease is handled now (fc3556cff).
- **A cancelled workflow body runs to completion,** and its outside effects aren't rolled back.
- **The approval card can appear before the drive commits its park** (agent-merge-approval). Not investigated.
- **The hosted-agent worker reads `local_tenant` only at startup.** Tenants placed later aren't driven until a restart.
- **sqlx 0.8.6 has no client TCP keepalive.** The listener relies on its 10s ping.

## Accounts and roles

- **SSO re-link:** a returning (re-invited) person's SSO credential link stays on their old principal. There's no command to re-link it.
- **The QUIT stall:** a relay that accepts DATA and then stalls on QUIT gets the letter twice (documented).
- **A cross-organization operator-join race:** the later join fails with `account_exists`.
- **The default role:** an owner link sent to a mistyped address blocks the owner default until it expires.

## Test suites (agreed plan, see the memory note on system tests)

- **System tests through the web server (no browser):** sessions, proxies, the CSRF guard, and web decoders run against real edge responses.
- **A small real-stack browser suite** for the main UI journeys plus axe.
- **Move mocked UI-state cases** to unit/component tests, then retire the dev-edge mock.
- **Get `fed test:system` into dogfood CI.** Scope the cost first.
- **Known failing mocked spec:** `issues.spec.ts` "issue create retries stay safe" (stale copy since 5c4e74219).
- **Known flaky or failing backend tests:**
  - `integration_outbox_quarantine` (Published(3) vs 2) fails at HEAD;
  - `integration_agent_trigger_durable` "another tenant's loops stay asleep" flakes, roughly 8 of 12 runs failing on base too;
  - `integration_mr009_kill9` is raced by the running outbox publisher on the shared DB.
