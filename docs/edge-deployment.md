# Edge deployment

The Rust edge ships as a stripped native Linux release bundle because its Git smart-HTTP wire
launches rootless gVisor and enforces memory limits through delegated cgroup v2. An ordinary nested
application container does not honestly provide that host contract.

Build the bundle from a clean, trusted checkout:

```sh
scripts/build-edge-release.sh
```

The script uses the committed Cargo lockfile and hardened release profile, then writes a deterministic
`target/release-bundles/myelin-edge-<revision>-<target>.tar.gz`, an archive checksum, and checksums for
the binary and this runbook. Verify both checksum layers before installation. Install `edge` as a
root-owned, non-writable executable and run it as a dedicated unprivileged service account.

The host must provide all of the following separately:

- Linux with a unified cgroup-v2 hierarchy and the `memory` controller delegated to the service.
- A glibc-compatible userspace with `libz`, `libgcc_s`, and the standard C/math runtime available.
- A pinned `runsc` installation. Set `MYELIN_RUNSC_BIN` to its absolute path.
- An immutable, root-owned Git guest rootfs containing executable `usr/bin/git`. Set
  `MYELIN_GVISOR_GIT_ROOTFS` to its absolute path.
- A persistent, pre-created `MYELIN_GIT_ROOT` outside temporary storage. Back it up with PostgreSQL
  and the KMS seal key; losing any one of those tiers prevents complete recovery.
- Private network access to PostgreSQL and the other endpoints required by the shared configuration.

Startup validates the runsc identity, guest Git, a real bounded cgroup, split PostgreSQL roles, every
migration, the durable KMS/cell roots, and Git recovery before binding. A failed prerequisite exits
non-zero; there is no development fallback in the production binary.

Store the environment in a root-readable `0600` secret file or an equivalent secret manager. At
minimum, serving requires:

```dotenv
DATABASE_URL=postgres://runtime-role:password@postgres.internal/myelin
DATABASE_MIGRATION_URL=postgres://migration-role:password@postgres.internal/myelin
MYELIN_CELL_ID=cell-eu-1
MYELIN_KMS_SEAL_KEY=<64 lowercase hex characters>
MYELIN_GIT_ROOT=/var/lib/myelin/git
MYELIN_RUNSC_BIN=/opt/myelin/bin/runsc
MYELIN_GVISOR_GIT_ROOTFS=/opt/myelin/git-rootfs
MYELIN_EDGE_ADDR=127.0.0.1:8080
MYELIN_PUBLIC_BASE_URL=https://myelin.example
MYELIN_ISSUES_RECONCILE_TENANTS=acme
MYELIN_REGION=fr-par
S3_ENDPOINT=https://s3.fr-par.example
S3_REGION=fr-par
S3_ACCESS_KEY=<secret>
S3_SECRET_KEY=<secret>
S3_BUCKET=myelin
REDIS_URL=rediss://valkey.internal:6380
NATS_URL=tls://nats.internal:4222
```

Terminate with `SIGTERM`. The listener stops accepting sockets, gracefully drains active HTTP
connections for 20 seconds, forcibly closes any remaining streams, drains the Issues reconciler, and
exits zero. Configure the supervisor termination deadline above 20 seconds.

Probe unauthenticated `GET /livez` for process liveness and `GET /readyz` for traffic readiness.
Readiness performs coalesced, bounded PostgreSQL and durable-Git write/sync checks. A dependency outage
returns 503 while liveness remains 200, preventing restart storms. Put the listener behind a private
TLS-terminating ingress; do not expose the raw edge port directly to the internet.

The listener admits at most 1,024 sockets, allows 10 seconds to finish request headers, accepts at
most 64 headers with a 64 KiB HTTP buffer, and rejects excess sockets immediately. Ordinary API bodies
are capped at 1 MiB. Only `git-receive-pack` receives the 100 MiB body budget. At most four Git wire
operations (advertise, fetch, or push) run concurrently; they use the runtime's blocking pool so a
sandboxed Git process cannot stall health probes or ordinary API traffic. Excess wire work returns 503.
Capacity responses carry `Retry-After: 1`; failed readiness carries `Retry-After: 5`, giving callers
bounded retry guidance instead of encouraging immediate retry storms.
Ordinary request bodies must finish within 30 seconds; Git push bodies receive a five-minute absolute
deadline. These are total body-read deadlines, so periodic trickle bytes do not retain intake slots.
An interrupted or malformed body stream returns a canonical 400 and closes the connection; partial
bytes are never reinterpreted as an empty request.
SIGINT or SIGTERM also propagates into active Git wire sandboxes: while HTTP connections drain, the
edge kills and reaps `runsc` containers instead of waiting for their two-minute operation limit.

Handler panics are isolated to the request and return the generic canonical 500 envelope. Production
logs suppress panic payloads so request-derived secrets cannot leak through Rust's default panic hook.

Every parsed request receives a fresh server-generated `X-Request-Id`. The edge writes one JSON
completion record containing only that ID, a bounded method and route class, status, and duration; it
never records raw paths or credentials. Connection-limit shedding is logged at power-of-two totals to
remain visible without enabling log amplification during a socket flood.

Every response disables content-type sniffing. API responses, errors, and health probes default to
`Cache-Control: no-store`; explicit protocol policies remain authoritative, including Git smart
HTTP's revalidation policy and SSE's `no-cache` stream policy.
If handler response metadata cannot be rendered as valid HTTP, the adapter fails closed with the
generic canonical 500 envelope; invalid metadata can never degrade into a successful empty response.
