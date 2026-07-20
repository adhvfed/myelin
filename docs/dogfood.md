# Founder dogfood runbook (R4.0)

This is how one operator runs the real Myelin `edge` binary against the dev stack, mints a capability
token, and uses it from `git` / the CLI / (soon) the web UI. Before R4.0 nothing could authenticate
against the real edge — the capability-token cell authority was ephemeral per boot and there was no
mint path. R4.0 makes the cell authority **durable** (sealed under the operator seal key) and adds the
`edge bootstrap` operator subcommand.

## 0. The seal key — READ THIS FIRST

`MYELIN_KMS_SEAL_KEY` (32 bytes as 64 hex chars) is the **root of trust**. It unseals BOTH:

- the **KMS root** (every per-tenant/per-subject data-encryption key), and
- the **capability-token cell root** (the Ed25519 signing seed + macaroon MAC key).

`scripts/dogfood.sh` generates it **once** into `${XDG_STATE_HOME:-$HOME/.local/state}/myelin/seal.key`
(mode `0600`) and reuses it. **Lose the seal key and you lose everything:** every encrypted column
becomes unrecoverable ciphertext AND every token you ever minted stops verifying. It is fail-closed by
design — a wrong/absent key makes the edge refuse to start; it NEVER regenerates a root over an
existing one. **Back it up** somewhere safe and out of the repo (e.g. a password manager):

```sh
cat "${XDG_STATE_HOME:-$HOME/.local/state}/myelin/seal.key"   # copy this to a safe place
```

## 1. Bring the data stack up

```sh
./scripts/dev-stack.sh up      # Postgres :5433 / Valkey :6380 / NATS :4222 / S3 :9000
```

## 2. Bring the edge up

```sh
./scripts/dogfood.sh edge      # builds + serves the edge on 127.0.0.1:8080 (durable-by-default)
```

The edge applies all migrations (incl. the R4.0 `cell_token_root`), loads the durable KMS root + the
durable cell token authority, and serves. It stays in the foreground; run the next steps in a second
terminal. `scripts/dogfood.sh env` keeps `DATABASE_URL` on the constrained `myelin_app` role and
supplies `DATABASE_MIGRATION_URL` as `myelin_admin`; the edge closes the latter before binding.

## 3. Mint an operator token (`edge bootstrap`)

```sh
PROJECT_UUID=20aee030-c7fa-4757-8243-700faf528690
./scripts/dogfood.sh bootstrap -- \
  --tenant acme --principal founder --issues-project "$PROJECT_UUID"
# → the capability token is printed to STDOUT (and NOTHING else — never logged / written to a file):
#   v4.public.<...>|<...>|<...>
```

Flags: `--tenant <slug>` (required), `--principal <id>` (required), `--issues-project <canonical UUID>`
(required), `--display-name <s>` (optional), `--ttl-days <n>` (default 30), `--region <r>` (default
`MYELIN_REGION`). Before printing a token, bootstrap idempotently writes exactly
`project:<uuid>#reader@<principal>` through the durable Identity tuple store. It does not grant project
writer/admin or tenant-wide authority, and Issues creation still passes the ordinary server-side
`may_create` project-view check. Running bootstrap again mints a **new** token while converging on the
same one reader tuple.

The edge's issue-authorization restart scanner is deliberately partitioned. Dogfood defaults
`MYELIN_ISSUES_RECONCILE_TENANTS` to this runbook's `acme` tenant. If you bootstrap a different or
additional tenant, export the complete comma-separated tenant list before starting `dogfood.sh edge`;
otherwise new issues in that tenant remain safely pending and invisible rather than activating.

Capture the token in a shell var for the next steps:

```sh
PROJECT_UUID=20aee030-c7fa-4757-8243-700faf528690
TOKEN="$(./scripts/dogfood.sh bootstrap -- \
  --tenant acme --principal founder --issues-project "$PROJECT_UUID" 2>/dev/null)"
```

The token is a **machine `agent` capability token** (no DPoP, no scope ceiling) whose signed purpose
is `OperatorBootstrap` and whose exact authority is `edge.operator`. That operator authority satisfies
the action-policy conjunct for the product surface, while every object-scoped action must still pass
its independent per-object ReBAC conjunct. The edge default token scheme is `agent`, so clients need
**no** extra scheme header.

### The operator trust boundary

Anyone with both database credentials (`DATABASE_URL` and `DATABASE_MIGRATION_URL`) **and** the seal
key can run `edge bootstrap` and mint a token for any principal in any tenant. That is accepted
operator-plane infrastructure. There is **deliberately no HTTP endpoint that mints** — minting is an
action on the box, never network-reachable.

## 4. Use the token — CLI

```sh
myelin login --token "$TOKEN" --scheme agent
myelin whoami
```

Or per-invocation: `myelin --token "$TOKEN" --scheme agent whoami`, or `export MYELIN_TOKEN="$TOKEN"`.

### Founder Issues loop

The canonical dogfood values are explicit because the current Issues spine has no type catalogue/FK:

```sh
PROJECT_UUID=20aee030-c7fa-4757-8243-700faf528690
TYPE_UUID=7d457754-f6a1-4cd8-8738-21751570b627
PREFIX=MYL
```

They are also exported by `./scripts/dogfood.sh env` as `MYELIN_DOGFOOD_ISSUES_PROJECT`,
`MYELIN_DOGFOOD_ISSUES_TYPE`, and `MYELIN_DOGFOOD_ISSUES_PREFIX`. The bootstrap command above grants
the founder reader access to this exact project. Start the edge with the default
`MYELIN_ISSUES_RECONCILE_TENANTS=acme`; it is the explicit FORCE-RLS partition list the restart-safe
Issues authorization worker scans.

Run the founder commands:

```sh
# The quoted title is one CLI argument. The create body is exactly project_id/type_id/prefix/title.
myelin issues create \
  --project "$PROJECT_UUID" --type "$TYPE_UUID" --prefix "$PREFIX" \
  --title "Ship the founder Issues floor"

# create returns a 202 receipt with authorization=pending and an Issue UUID. Copy that UUID here.
ISSUE_ID=<uuid-from-the-receipt>

# Pending rows are deliberately invisible. Retry view after the reconciler activates the tuple.
myelin issues view "$ISSUE_ID"
myelin issues list --state open --limit 25
myelin issues list --state all --key "$PREFIX-" --limit 25
myelin issues close "$ISSUE_ID"
myelin issues view "$ISSUE_ID"
```

The CLI never claims a 202 receipt is immediately visible; it prints the matching `myelin issues view
<uuid>` command. A very early `view` can return the same leak-free 404 as an unauthorized/absent issue;
retry after the worker's default five-second sweep. A viewer-capable client can use the receipt's
`authorization.request_event_id` at `/v1/issues/authorization-requests/<request_event_id>`: it returns
`202` plus a bounded retry hint while pending and `200` plus the full Issue view after activation. The
create response deliberately has no `Location` header because a create-only credential has no
`issue.view` authority; the status endpoint never weakens that boundary or exposes retry errors and
attempt counts. The worker scans durable pending bindings directly. The separate outbox relay is not
required to activate the issue and is not started implicitly here.

`issues list` defaults to `--state open`; use `closed` or `all` explicitly for the other views. `--key`
is a normalized Issue-key prefix filter (for example `MYL-`), never a title or free-text search. Keep
the same `--state` and `--key` values when passing the opaque `--cursor` printed for the next page.

## 5. Use the token — `git`

The git smart-HTTP wire accepts the token two ways.

**A. HTTP Basic (git-native), via a credential helper — the password is the token, the username is
ignored:**

```sh
# clone (create the repo first, e.g. via `myelin git repo create acme/eu-west/widgets` or the JSON API)
git clone http://127.0.0.1:8080/acme/eu-west/widgets.git
# when prompted (or configure a helper): username = anything, password = <TOKEN>

# non-interactive credential helper (password = token):
git -c credential.helper='!f() { echo "username=x-access-token"; echo "password='"$TOKEN"'"; }; f' \
    clone http://127.0.0.1:8080/acme/eu-west/widgets.git
```

**B. The `http.extraHeader` alternative** (Bearer OR a pre-formed Basic header — handy for CI):

```sh
# Bearer:
git -c http.extraHeader="Authorization: Bearer $TOKEN" \
    clone http://127.0.0.1:8080/acme/eu-west/widgets.git

# or a pre-formed Basic header (base64 of "user:token"):
git -c http.extraHeader="Authorization: Basic $(printf 'x:%s' "$TOKEN" | base64 -w0)" \
    push  http://127.0.0.1:8080/acme/eu-west/widgets.git main
```

Push identities must be pseudonymous handles for the tenant (the GIT-1 push gate), e.g.:

```sh
git config user.email "founder@acme.noreply"
git config user.name  "founder@acme.noreply"
```

> Only the git **wire** routes accept HTTP Basic. The JSON product API (`/v1/git/...`, `/v1/whoami`)
> is **Bearer-only** — send `Authorization: Bearer $TOKEN` (the CLI does this for you).

## 6. Use the token — web

The edge exposes an operator-token login gate at `GET /v1/auth/config` (`token_login_enabled`), turned
on by `MYELIN_TOKEN_LOGIN=1` (the dogfood env sets it). The `/login` page renders a token card: paste
your `edge bootstrap` token to sign in (it is verified server-side against the edge `whoami`). Point the
web app at the edge:

```sh
./scripts/dogfood.sh web       # prints the env + instructions (EXEC=1 to actually start pnpm/vinxi)
```

The production web server emits its CSP and browser-security headers automatically. Set
`MYELIN_HSTS=1` only after the public hostname is permanently HTTPS; this opts that hostname into a
one-year HSTS policy. Leave it unset for local HTTP and initial TLS canaries.

## 6a. Solo-operator merges (branch protection)

A fresh repo's default ruleset requires **1 approval**, and Myelin does **not** count an author's approval
of their own PR (a deliberate policy). A solo founder therefore cannot merge until a second reviewer exists.
For single-operator dogfood, set the ref's required approvals to 0 (a repo-admin write):

```sh
curl -sS -X POST "$MYELIN_EDGE_URL/v1/git/repos/<repo>/branch-protection" \
  -H "authorization: Bearer $TOKEN" -H "content-type: application/json" \
  -d '{"rulesets":[{"ref_pattern":"refs/heads/main","required_approvals":0,"required_contexts":[]}]}'
```

> Open PRs with an explicit `head_oid` (`git rev-parse <head_ref>`) — a PR opened without it currently
> cannot be merged. Mirroring an existing GitHub repo's *history* is rejected by the pseudonymous-commit
> gate (real emails are not pseudonyms); dogfood new work with a `<handle>@<tenant>.noreply` identity and
> keep GitHub as the read-only history mirror.

## 7. Revoke a token

```sh
cargo run -p myelin-edge --bin edge -- revoke --jti <jti> --tenant acme
# (the jti is printed on stderr by `edge bootstrap`; the S7 denylist is (tenant, region)-partitioned,
#  so revoke names the token's tenant — and the deny survives a restart.)
```
