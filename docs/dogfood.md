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
terminal.

## 3. Mint an operator token (`edge bootstrap`)

```sh
./scripts/dogfood.sh bootstrap -- --tenant acme --principal founder
# → the capability token is printed to STDOUT (and NOTHING else — never logged / written to a file):
#   v4.public.<...>|<...>|<...>
```

Flags: `--tenant <slug>` (required), `--principal <id>` (required), `--display-name <s>` (optional),
`--ttl-days <n>` (default 30), `--region <r>` (default `MYELIN_REGION`). Running it again for the same
principal mints a **new** token (a new revocation id) without disturbing the principal.

Capture the token in a shell var for the next steps:

```sh
TOKEN="$(./scripts/dogfood.sh bootstrap -- --tenant acme --principal founder 2>/dev/null)"
```

The token is a **machine `agent` capability token** (no DPoP, no scope ceiling) with the `agent:run`
grant — sufficient for the full product surface (repo create, git pull/push, PR open/review/merge,
whoami, events). The edge default token scheme is `agent`, so clients need **no** extra scheme header.

### The operator trust boundary

Anyone with the `DATABASE_URL` credentials **and** the seal key can run `edge bootstrap` and mint a
token for any principal in any tenant. That is accepted operator-plane infrastructure (the same
boundary the seal key already draws). There is **deliberately no HTTP endpoint that mints** — minting
is an action on the box, never network-reachable.

## 4. Use the token — CLI

```sh
myelin login --token "$TOKEN" --scheme agent
myelin whoami
```

Or per-invocation: `myelin --token "$TOKEN" --scheme agent whoami`, or `export MYELIN_TOKEN="$TOKEN"`.

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

## 6. Use the token — web (coming in the frontend half)

The edge exposes an operator-token login gate at `GET /v1/auth/config` (`token_login_enabled`), turned
on by `MYELIN_TOKEN_LOGIN=1` (the dogfood env sets it). The web form that consumes it — paste your
`edge bootstrap` token to sign in — is the **separate R4.0 frontend deliverable**. Point the web app
at the edge meanwhile:

```sh
./scripts/dogfood.sh web       # prints the env + instructions (EXEC=1 to actually start pnpm/vinxi)
```

## 7. Revoke a token

```sh
cargo run -p myelin-edge --bin edge -- revoke --jti <jti> --tenant acme
# (the jti is printed on stderr by `edge bootstrap`; the S7 denylist is (tenant, region)-partitioned,
#  so revoke names the token's tenant — and the deny survives a restart.)
```
