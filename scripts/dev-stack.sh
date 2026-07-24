#!/usr/bin/env bash
# Myelin dev data-layer stack helper (Stage 1).
#
#   ./scripts/dev-stack.sh up      bring the stack up and block until all healthchecks pass
#   ./scripts/dev-stack.sh down    stop and remove the stack (keeps named volumes)
#   ./scripts/dev-stack.sh nuke    stop and remove the stack AND its volumes (data loss)
#   ./scripts/dev-stack.sh wait    re-run the health gate (up --wait) without recreating
#   ./scripts/dev-stack.sh ps      show service health
#   ./scripts/dev-stack.sh logs    follow service logs (optionally a service name arg)
#   ./scripts/dev-stack.sh env     print the dev env-var contract (eval-able)
#
# Stack: postgres:16 + rustfs + valkey:8 + nats:2.10 (JetStream). See docs/dev-stack.md.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/docker-compose.dev.yml"
COMPOSE=(docker compose -f "${COMPOSE_FILE}")

cmd="${1:-up}"; shift || true

case "${cmd}" in
  up)
    "${COMPOSE[@]}" up -d --wait
    "${COMPOSE[@]}" ps
    ;;
  wait)
    "${COMPOSE[@]}" up -d --wait --no-recreate
    "${COMPOSE[@]}" ps
    ;;
  down)
    "${COMPOSE[@]}" down
    ;;
  nuke)
    "${COMPOSE[@]}" down -v
    ;;
  ps)
    "${COMPOSE[@]}" ps
    ;;
  logs)
    "${COMPOSE[@]}" logs -f "$@"
    ;;
  env)
    cat <<'EOF'
export DATABASE_URL="postgres://myelin_app:myelin_app_pw@localhost:5433/myelin"
export DATABASE_MIGRATION_URL="postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin"
export MYELIN_CI_SCHEDULER_DATABASE_URL="postgres://myelin_ci_scheduler_fr_par:myelin_ci_scheduler_dev_pw@localhost:5433/myelin"
export MYELIN_OUTBOX_PUBLISHER_DATABASE_URL="postgres://myelin_outbox_publisher_fr_par:myelin_outbox_publisher_dev_pw@localhost:5433/myelin"
export MYELIN_OUTBOX_PROVISION_NATS_URL="nats://localhost:4222"
export MYELIN_OUTBOX_PUBLISH_NATS_URL="nats://localhost:4222"
export MYELIN_OUTBOX_PUBLISHER_BATCH="4"
export MYELIN_OUTBOX_PUBLISHER_POLL_MS="100"
export MYELIN_OUTBOX_PUBLISHER_BACKOFF_MS="500"
export MYELIN_OUTBOX_PUBLISHER_STATEMENT_TIMEOUT_MS="1000"
export MYELIN_OUTBOX_PUBLISHER_MAX_ENVELOPE_BYTES="262144"
export MYELIN_OUTBOX_STREAM_MAX_AGE_SECONDS="7776000"
export MYELIN_OUTBOX_STREAM_MAX_BYTES="67108864"
export MYELIN_OUTBOX_STREAM_MAX_MESSAGES="100000"
export MYELIN_OUTBOX_STREAM_REPLICAS="1"
export MYELIN_OUTBOX_STREAM_DEDUP_SECONDS="120"
export MYELIN_OUTBOX_PUBLISH_ACK_TIMEOUT_MS="2000"
export MYELIN_OUTBOX_PUBLISHER_PASS_TIMEOUT_MS="20000"
export S3_ENDPOINT="http://localhost:9000"
export S3_REGION="fr-par"
export S3_ACCESS_KEY="myelin_dev_access"
export S3_SECRET_KEY="myelin_dev_secret"
export S3_BUCKET="myelin-dev"
export REDIS_URL="redis://localhost:6380"
export NATS_URL="nats://localhost:4222"
export MYELIN_REGION="fr-par"
EOF
    ;;
  *)
    echo "usage: $0 {up|wait|down|nuke|ps|logs|env}" >&2
    exit 2
    ;;
esac
