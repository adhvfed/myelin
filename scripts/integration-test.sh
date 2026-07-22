#!/usr/bin/env bash
# Myelin INTEGRATION GATE (Stage 4) — the band-boundary gate over the REAL backends.
#
# This is the COMMITTED ratchet for the testing-policy change: every DB / storage / cache / bus
# prompt ships a REAL integration test, and the infra scorecard rows stay RED-until-proven — a row
# only reads green once its `cargo test --features integration` emits a dated green artifact
# against the LIVE docker-compose stack (Postgres / RustFS / Valkey / NATS JetStream).
#
# It:
#   1. brings the dev stack up and BLOCKS until every healthcheck passes (`up -d --wait`);
#   2. runs the workspace integration suite (`cargo test --features integration`) — including the
#      four retrofitted drills (outbox-no-loss, restore-verify, RLS-isolation, ReBAC-no-leak) and
#      the two genuine-floor containerized smokes (hardened-container sandbox + 10× load);
#   3. runs the infra scorecard runner (records each row, writes testing/scorecards/infra.md, and
#      EXITS NON-ZERO if any row is missing or claimed-not-proven);
#   4. re-arms the make-it-real evidence spine against the still-live stack and refreshes
#      thresholds.toml's `as_of` only after every attested proof is green;
#   5. optionally tears the stack down (`--down`) — by default it is LEFT UP for re-runs.
#
# There is deliberately NO `|| true` / swallow path: a red drill OR a red scorecard fails the gate.
#
# Usage:
#   scripts/integration-test.sh            # up --wait → test → scorecard (leaves stack up)
#   scripts/integration-test.sh --down     # also tear the stack down at the end
#   scripts/integration-test.sh --nuke     # tear the stack down AND drop volumes at the end
#
# Prod is a CONFIG SWAP, not a code change: the same `--features integration` suite runs against
# Scaleway (fr-par) Managed PostgreSQL / Object Storage / Managed Redis + the NATS container by
# pointing DATABASE_URL / S3_* / REDIS_URL / NATS_URL / MYELIN_REGION at the prod endpoints.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/docker-compose.dev.yml"
COMPOSE=(docker compose -f "${COMPOSE_FILE}")

TEARDOWN="${1:-none}"

echo "==> [1/4] bringing the dev data-layer stack up (blocks until healthchecks pass) …"
"${COMPOSE[@]}" up -d --wait
"${COMPOSE[@]}" ps

cleanup() {
  case "${TEARDOWN}" in
    --down) echo "==> tearing the stack down (keeping volumes) …"; "${COMPOSE[@]}" down ;;
    --nuke) echo "==> tearing the stack down AND dropping volumes …"; "${COMPOSE[@]}" down -v ;;
    *)      echo "==> leaving the stack UP (pass --down or --nuke to tear it down)" ;;
  esac
}
trap cleanup EXIT

# The dev env-var contract the myelin-config layer points at (same values as scripts/dev-stack.sh).
export DATABASE_URL="${DATABASE_URL:-postgres://myelin_app:myelin_app_pw@localhost:5433/myelin}"
export S3_ENDPOINT="${S3_ENDPOINT:-http://localhost:9000}"
export S3_REGION="${S3_REGION:-fr-par}"
export S3_ACCESS_KEY="${S3_ACCESS_KEY:-myelin_dev_access}"
export S3_SECRET_KEY="${S3_SECRET_KEY:-myelin_dev_secret}"
export S3_BUCKET="${S3_BUCKET:-myelin-dev}"
export REDIS_URL="${REDIS_URL:-redis://localhost:6380}"
export NATS_URL="${NATS_URL:-nats://localhost:4222}"
export MYELIN_REGION="${MYELIN_REGION:-fr-par}"

echo
echo "==> [2/4] running the workspace integration suite (cargo test --features integration) …"
# --features integration is a per-crate feature; running it workspace-wide compiles + runs every
# crate's real-backend tests. The four retrofitted drills + the two floor smokes live in
# myelin-storage; the bus/cache integration tests live in myelin-events / myelin-storage.
cargo test --workspace --features integration -- --nocapture

echo
echo "==> [3/4] running the infra scorecard (red-until-proven; writes testing/scorecards/infra.md) …"
cargo run -p myelin-harness --bin infra-scorecard

echo
echo "==> [4/4] re-arming the make-it-real evidence spine (writes JSON + derived Markdown) …"
cargo run -p myelin-harness --bin make-it-real-scorecard -- --refresh-thresholds-as-of

echo
echo "==> INTEGRATION GATE GREEN — every infra integration drill proven against the live stack."
echo "    (the two named floors — real-kernel SANDBOX-ESCAPE + WORLD-SCALE 30× — stay open by design.)"
