#!/usr/bin/env bash
# Compatibility wrapper around the Fed development stack.
#
#   ./scripts/dev-stack.sh up      bring the stack up and block until all healthchecks pass
#   ./scripts/dev-stack.sh down    stop and remove the stack (keeps named volumes)
#   ./scripts/dev-stack.sh nuke    stop and remove the stack AND its volumes (data loss)
#   ./scripts/dev-stack.sh wait    ensure every dependency is running and healthy
#   ./scripts/dev-stack.sh ps      show service health
#   ./scripts/dev-stack.sh logs    follow service logs (optionally a service name arg)
#   ./scripts/dev-stack.sh env     print service URLs for the current Fed port allocation
#
# `fed start` is the canonical interface; this wrapper remains for existing operator habits.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

cmd="${1:-up}"; shift || true

fed_port() {
  local name="$1"
  local port
  port="$(fed ports list | awk -v name="${name}" '$2 == name { print $1 }')"
  if [[ -z "${port}" ]]; then
    echo "No Fed port is allocated for ${name}; run 'fed start' first." >&2
    exit 1
  fi
  printf '%s' "${port}"
}

case "${cmd}" in
  up)
    fed start
    ;;
  wait)
    fed start
    ;;
  down)
    fed stop
    ;;
  nuke)
    fed stop
    fed clean
    ;;
  ps)
    fed status
    ;;
  logs)
    fed logs "$@"
    ;;
  env)
    postgres_port="$(fed_port POSTGRES_PORT)"
    s3_port="$(fed_port S3_PORT)"
    valkey_port="$(fed_port VALKEY_PORT)"
    nats_port="$(fed_port NATS_PORT)"
    edge_port="$(fed_port EDGE_PORT)"
    web_port="$(fed_port WEB_PORT)"
    cat <<EOF
export DATABASE_URL="postgres://myelin_app:myelin_app_pw@localhost:${postgres_port}/myelin"
export DATABASE_MIGRATION_URL="postgres://myelin_admin:myelin_dev_pw@localhost:${postgres_port}/myelin"
export MYELIN_CI_SCHEDULER_DATABASE_URL="postgres://myelin_ci_scheduler_fr_par:myelin_ci_scheduler_dev_pw@localhost:${postgres_port}/myelin"
export MYELIN_OUTBOX_PUBLISHER_DATABASE_URL="postgres://myelin_outbox_publisher_fr_par:myelin_outbox_publisher_dev_pw@localhost:${postgres_port}/myelin"
export MYELIN_OUTBOX_PROVISION_NATS_URL="nats://localhost:${nats_port}"
export MYELIN_OUTBOX_PUBLISH_NATS_URL="nats://localhost:${nats_port}"
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
export S3_ENDPOINT="http://localhost:${s3_port}"
export S3_REGION="fr-par"
export S3_ACCESS_KEY="myelin_dev_access"
export S3_SECRET_KEY="myelin_dev_secret"
export S3_BUCKET="myelin-dev"
export REDIS_URL="redis://localhost:${valkey_port}"
export NATS_URL="nats://localhost:${nats_port}"
export MYELIN_REGION="fr-par"
export MYELIN_EDGE_URL="http://127.0.0.1:${edge_port}"
export MYELIN_WEB_URL="http://127.0.0.1:${web_port}"
EOF
    ;;
  *)
    echo "usage: $0 {up|wait|down|nuke|ps|logs|env}" >&2
    exit 2
    ;;
esac
