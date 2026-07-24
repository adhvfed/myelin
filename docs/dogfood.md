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

The edge also requires an explicit `MYELIN_CELL_ID` and `MYELIN_GIT_ROOT` before it touches the
database. The Git root must already exist as an absolute directory outside the operating-system temp
directory; mount it on durable storage and include it in the backup plan. Startup refuses a missing,
relative, root-level, temporary, or nonexistent path instead of silently placing repositories under
`/tmp`. `scripts/dogfood.sh` creates its persistent XDG data directory and exports both values.
Serving also requires explicit absolute `MYELIN_RUNSC_BIN` and `MYELIN_GVISOR_GIT_ROOTFS` paths. The
rootfs must contain an executable `usr/bin/git`; a missing gVisor runtime or guest Git now fails
startup instead of deferring a broken clone/push path until the first request. The host must use
cgroup v2 and delegate the `memory` controller: serving startup creates and removes a real bounded
probe cgroup before touching PostgreSQL, then refuses if Git wire workloads could not be memory-capped.

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

Orchestrators may probe `GET /livez` and `GET /readyz` without credentials. Liveness reports only
whether the HTTP process can respond; it deliberately ignores downstream outages. Readiness performs
bounded checks against PostgreSQL and the writable durable Git root, returning `503` with no internal
error details when either critical dependency cannot serve. During termination the edge stops
accepting sockets, drains HTTP connections for up to 20 seconds, then closes any remaining streams.

### Bring the CI runner up

CI dogfood has four independently restartable service roots. Start the publisher first so its
separate provision authority can create or validate the bounded shared stream before any consumer
binds, then start each remaining root in its own foreground-owned terminal:

```sh
./scripts/dogfood.sh publisher     # elected PostgreSQL outbox → bounded shared JetStream
./scripts/dogfood.sh dispatch      # Git events → durable CI runs + initial queued checks
./scripts/dogfood.sh ci            # coordinated CI control plane and runner host
./scripts/dogfood.sh git-checks    # ci.check.updated → Git-owned durable projection
```

The `check:v1-<BLAKE3>` aggregate is a deliberate broker-routing cutover. Events emitted by an older
build used the raw repository ArtifactRef as their aggregate and are not projection backfill
authority. For an existing cell, keep the old Edge serving only as the rerun/verification front
door, deploy the new publisher, `git-checks`, Dispatch, and coordinated CI/control-plane builds,
drain every old Dispatch or CI producer instance, and start all four new service roots. Only then
trigger a fresh CI rerun for every protected head and verify each exact OID with `verify-check`.
Deploy the new Edge
build that reads only the projection after every verification passes. Abort that Edge rollout if any
producer cannot be upgraded/drained or any protected head cannot be re-established. A new
personal-production cell has no historical check state and needs no backfill. This is fail-closed:
old or in-flight facts can strand a head blocked, but can never make it green.

All four use the same dogfood cell, region, durable bus, and data services. The publisher connects
through a dedicated one-connection database capability that can only read the shared outbox, update
its `published_at` column, and insert payload-free quarantine rows. Provisioning and runtime publish
authority are separate: startup provisions or validates the finite stream, while the long-running
publisher can only publish to that existing stream. Dispatch consumes the production Git event
stream and owns reserve/start. The control-plane command sets the exact opt-in
`MYELIN_CI_RUNNER=1`; its host owns queued-run start,
durable Flow recovery, real gVisor execution, terminal accounting, and bounded shutdown as one
lifecycle. Git's projection consumer applies every check fact and its consumer-dedup mark in one
transaction; the edge reads that Git-owned table rather than calling CI or trusting a PR mutation.
Unset or `MYELIN_CI_RUNNER=0` keeps the runner lanes dormant; any other value is refused before
database access.
Activation also refuses before database access unless `MYELIN_RUNSC_BIN` is an absolute executable
that identifies itself as gVisor `runsc`, `MYELIN_GVISOR_ROOTFS` is an absolute staged base rootfs
with the required executables, and the host can execute a bounded non-root `/bin/false` smoke through
the real rootless runsc, read-only OCI bundle, and delegated cgroup-v2 memory boundary.
Before starting intake, `dogfood.sh ci` also recomputes a deterministic canonical-tree SHA-256 of
that staged rootfs and requires it to match the image digest in the checked-in `.myelin/ci.toml`.
Run `./scripts/dogfood.sh verify-ci-rootfs` for the same read-only preflight explicitly; a mismatch
must be resolved by intentionally restaging, redrilling, and updating the pin, never by weakening the
comparison.
Keep the process in the foreground during dogfood. Stop it with SIGINT/SIGTERM and require a clean
drain before restarting or upgrading it. Do the same for dispatch and the Git projection consumer.

After pushing a branch, opening its PR, and waiting for CI, run the read-only surfaced-check proof
with the exact pushed commit. It performs only authenticated GETs; it neither changes the PR nor
manually reports a check:

```sh
export MYELIN_TOKEN="$TOKEN"
./scripts/dogfood.sh verify-check <repo> <pr-number> "$(git rev-parse <head-ref>)" build
```

The command canonicalizes the short `build` argument to `ci/build` and exits nonzero unless the PR
still points at that exact OID and that full context is in both the repository ruleset's required set
and Git's settled, trusted green projection. Its JSON result also reports the full merge-gate
verdict; the verdict may remain false until the separate review threshold is satisfied.

Capture the CI surface evidence from that same run; do not substitute a fixture or a different head.
While the pushed job is still running, discover its server-issued identifiers and attach both
consumers before it finishes:

```sh
set -o pipefail
EVIDENCE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/myelin/acceptance"
mkdir -p "$EVIDENCE_DIR"
myelin --json ci list --status running --limit 20
myelin ci view <run>
myelin ci watch <run> --job <job> | tee "$EVIDENCE_DIR/myelin-ci-live-<run>-<job>.log"
```

At the same time, open `/ci/runs/<run>` in the authenticated web app and observe the bounded recent
live output advance to completion. The run and job identifiers must come from the durable list/detail
responses; do not precompute or copy them from PostgreSQL. A job that finishes before either consumer
attaches does not prove live usability—push another harmless Myelin commit and repeat.
The checked-in founder pipeline makes this mechanically practical: it first emits
`myelin-ci-acceptance-window-open`, then waits for a bounded 120 seconds before emitting the unique
acceptance marker. Discover the server-issued run/job and attach both consumers during that window;
the delay is acceptance instrumentation, not a substitute for observing live bytes.

After completion, prove that the cold path has the same output and that the exact run settled:

```sh
myelin --json ci view <run> | tee "$EVIDENCE_DIR/myelin-ci-run-cli-<run>.json"
myelin ci logs <run> --job <job> --start 0 --limit 262144 \
  | tee "$EVIDENCE_DIR/myelin-ci-archive-cli-<run>-<job>.log"
./scripts/dogfood.sh verify-ci <run> <job> \
  'MYELIN-CI-a005e32fc1bb0c2b64e7d40ac1a01236' "$EVIDENCE_DIR"
```

Follow any parser-round-trippable `more — run:` command printed by the archive reader until no
continuation remains. `verify-ci` independently reads the terminal detail and every bounded archive
page through the production Edge, refuses a failed or unsettled run, validates exact scope and
contiguous byte coordinates, assembles the byte-exact archive, and requires the chosen marker exactly
once in both the earlier CLI live capture and durable archive. The deliberately non-self-overlapping
marker shape makes that count unambiguous. It refuses to overwrite prior evidence and writes a
checksum-bearing JSON receipt. Confirm the same marker appeared once in the browser and
CLI archive too. Preserve that receipt together with the `verify-check` JSON and exact pushed OID. Do
not mark the founder pass or start the R4 clock if the run fails, costs remain unsettled, the required
check is absent/stale, a consumer needed GitHub, or any log surface loses/duplicates the marker.
The marker is emitted exactly once by the checked-in `.myelin/ci.toml`; its compiled Dispatch
contract fails if that production config stops parsing, resolving, naming `build`, retaining its
rootfs digest pin, or carrying the exact marker once.

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
# clone (create the repo first, e.g. via
# `myelin --idempotency-key repo-create-widgets git repo create acme/eu-west/widgets` or the JSON API)
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

For the one-time Myelin source migration, do not push the legacy GitHub commit graph into an empty
Myelin lineage. Historical commits with non-pseudonymous identities are correctly rejected, and
rewriting them would destroy the GitHub archive's identity. Instead, make one new pseudonymous
snapshot commit whose parent is the hosted Myelin `main` and whose tree is the clean, reviewed
current source tree; GitHub remains the read-only historical mirror for the planned quarter. Run
`cargo test -p myelin-git self_hosting_tree_contains_no_complete_default_secret_sentinel` before
that snapshot. The test derives the production scanner's default patterns and fails if scanner or
redaction fixtures make the repository reject its own source blobs.

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
one-year HSTS policy. Leave it unset for local HTTP and initial TLS canaries. Production also requires
`MYELIN_PUBLIC_ORIGIN` (for example `https://myelin.example`) so every unsafe browser request can be
checked against the deployment's exact scheme, host, and port. It must be HTTPS and contain no path.
Production also requires a `rediss://` `REDIS_URL`: browser sessions—including bearer credentials—
are stored in the region-local Valkey service, so plaintext `redis://` is accepted only by local
development and tests. Set `MYELIN_WEB_SESSION_KEY` to the same 32-random-byte base64/base64url secret
on every web replica; trusted session records are AES-256-GCM encrypted and authenticated in Valkey,
and a key change logs out existing sessions. `MYELIN_EDGE_URL` is also mandatory in production and
must be an absolute HTTPS origin with no credentials, path, query, or fragment. Keep that edge origin
on the private service network; the loopback development edge is never a production fallback. Startup
fails before
accepting traffic when any required setting is missing or invalid. Use `/healthz` for process
liveness and `/readyz` for traffic readiness; the latter
performs a short-lived namespaced write/read/script/delete probe and returns 503 whenever the session
backend is unavailable or its production ACL cannot perform real session operations.

Interactive SSO additionally requires the web client settings documented in
[`web-deployment.md`](web-deployment.md) and the edge verifier settings documented in
[`edge-deployment.md`](edge-deployment.md). The registered redirect URI is the exact public origin
plus `/auth/oidc/callback`; Myelin uses a confidential authorization-code client with S256 PKCE.

The Node listener is an internal HTTP hop and must not be exposed publicly. Put it behind the TLS
ingress, keep the listener reachable only on the private service network, and configure the ingress
to **strip and replace** any client-supplied `X-Forwarded-Proto` header with the actual public scheme.
Production accepts only the exact value `https`: insecure GET/HEAD requests receive a 308 to
`MYELIN_PUBLIC_ORIGIN`, while insecure writes are rejected without replaying their bodies. Point
external health checks through that ingress (or include its trusted HTTPS assertion on private
probes); direct requests without the assertion intentionally do not reach application routes.
See [`web-deployment.md`](web-deployment.md) for the reproducible OCI build and hardened runtime
contract.

## 6a. Solo-operator merges (branch protection)

A fresh repo's default ruleset requires **1 approval**, and Myelin does **not** count an author's approval
of their own PR (a deliberate policy). A solo founder therefore cannot merge until a second reviewer exists.
For single-operator CI dogfood, set the ref's required approvals to 0 while retaining `ci/build` as
a required context (a repo-admin write):

```sh
curl -sS -X POST "$MYELIN_EDGE_URL/v1/git/repos/<repo>/branch-protection" \
  -H "authorization: Bearer $TOKEN" -H "content-type: application/json" \
  -d '{"rulesets":[{"ref_pattern":"refs/heads/main","required_approvals":0,"required_contexts":["ci/build"]}]}'
```

Do not replace `required_contexts` with an empty list for the founder acceptance run: that would
make `verify-check ... build` fail and, more importantly, would leave `main` without the CI gate the
run is meant to prove.

> Open PRs with an explicit `head_oid` (`git rev-parse <head_ref>`) — a PR opened without it currently
> cannot be merged. Mirroring an existing GitHub repo's *history* is rejected by the pseudonymous-commit
> gate (real emails are not pseudonyms); dogfood new work with a `<handle>@<tenant>.noreply` identity and
> keep GitHub as the read-only history mirror.

CLI mutations require the global `--idempotency-key`. Choose a PII-free key for the intended
operation and reuse that exact key if the response is lost; changing the key creates a distinct
operation. For example:

```sh
HEAD_OID="$(git rev-parse refs/heads/topic)"
myelin --idempotency-key "pr-open-topic-$HEAD_OID" git pr open <repo> \
  --title "Topic" --head-ref refs/heads/topic --head-oid "$HEAD_OID"
```

## 7. Revoke a token

```sh
cargo run -p myelin-edge --bin edge -- revoke --jti <jti> --tenant acme
# (the jti is printed on stderr by `edge bootstrap`; the S7 denylist is (tenant, region)-partitioned,
#  so revoke names the token's tenant — and the deny survives a restart.)
```
