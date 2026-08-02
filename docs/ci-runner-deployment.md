# CI runner (gVisor sandbox) host deployment

The CI control plane's runner lane (`ci-controlplane` with `MYELIN_CI_RUNNER=1`) launches untrusted
job commands inside a real `runsc` (gVisor) sandbox — the same no-host-exec seam the escape-drill
corpus (CT-003/AG-D4) proves closed. This doc covers ONLY the CI-sandbox/gVisor-specific host
requirements, following the same path conventions as `docs/edge-deployment.md`
(`/opt/myelin/...` for pinned binaries/immutable assets, `/var/lib/myelin/...` for mutable persistent
state); the rest of `ci-controlplane`'s configuration (PostgreSQL roles, cell id, etc.) is out of
scope here.

## Two activation levels

Both levels are `MYELIN_CI_RUNNER=1` — a second, independent env var,
`MYELIN_CI_WORKSPACE_MODE`, selects between them (`crates/myelin-ci-controlplane/src/main.rs`'s
`parse_workspace_activation_given`, parsed and preflighted exactly once at startup, before any
PostgreSQL bootstrap):

**Rootless (`MYELIN_CI_WORKSPACE_MODE` unset or `disabled`).** The runner boots every guest with
`runsc --rootless`, no host capabilities beyond an ordinary unprivileged process —
`runner_bind.rs`'s `GvisorBackend::try_new(..., GvisorWorkspaceConfig::Disabled, ...)` call.

**Explicit user-namespace + `EphemeralDisk` workspaces (`MYELIN_CI_WORKSPACE_MODE=enabled`, CT-007
slice 4).** Each job gets its own real subordinate uid/gid mapping (`UserNamespaceLease`) and a real
Btrfs-quota'd scratch subvolume (`ManagedWorkspace`) instead of an unbounded host-backed `/tmp`. This
mode requires four additional variables — `MYELIN_EXPLICIT_USERNS_RUNSC_ROOT`,
`MYELIN_USERNS_LEASES_DIR`, `MYELIN_CI_WORKSPACES_DIR`, `MYELIN_CI_WORKSPACE_CAPACITY_BYTES` (a
positive integer byte count with no default — an operator/storage-layout decision; see the systemd
unit's `Environment=` block for a worked example) — plus an optional
`MYELIN_EXPLICIT_USERNS_HELPER_DIR` (defaults to `/usr/bin`, where `newuidmap`/`newgidmap` live).
Startup refuses loudly if `enabled` is set and any of the four required variables is missing,
empty, non-absolute, or (for the capacity) not a positive integer — never a silent fallback to
`Disabled`. The HOST-side provisioning this needs (directories, the pinned `runsc` binary's
placement, the `CAP_SYS_ADMIN`/`CAP_CHOWN` grants, and cgroup delegation) is what this doc and
`scripts/install-ci-runner-host.sh` set up.

## Provisioning the host

```sh
sudo ./scripts/install-ci-runner-host.sh /path/to/your/pinned/runsc
```

This is idempotent — re-running it against an already-correct host only re-asserts ownership/mode,
never touches an existing `runsc-root` or `userns-leases` directory's contents (both hold durable
crash-recovery state). It:

- Creates a dedicated, unprivileged system service account (`myelin-runner` by default; override via
  `MYELIN_CI_RUNNER_USER`/`_GROUP`) — **never run this service as root.** Every explicit-userns
  hardening check (`harden_explicit_userns_runsc_binary`, `harden_and_verify_leases_dir`, ...)
  verifies its OWN ancestor chain is not writable by the process's effective uid; running as root
  makes every directory on the host "writable by us" by definition, which these checks correctly
  refuse (this is not a corner case — a session this deployment story was developed against hit it
  directly when testing as root instead of the service account).
- Installs the pinned `runsc` at `/opt/myelin/bin/runsc`, root-owned, mode `0755`, under a root-owned
  parent chain — satisfying `harden_explicit_userns_runsc_binary`'s ancestor-immutability
  requirement. The script does not itself verify the pinned digest; the running service refuses to
  boot against a mismatched one (`verify_pinned_explicit_userns_runsc`), which is the actual
  enforcement point and avoids keeping the pin in two places that could drift.
- Creates `/opt/myelin/gvisor-runsc-root` (the `runsc --root=` state directory) and
  `/var/lib/myelin/userns-leases` (the durable lease-marker directory), both owned by the service
  account, mode `0700` exactly — the shape `harden_explicit_userns_runsc_root` and
  `UserNamespaceAllocator::try_new`'s strict mode both require.
- Creates `/var/lib/myelin/ci-workspaces` for the `EphemeralDisk` workspace base directory and warns
  if it is not actually on a Btrfs mount (creating/mounting the Btrfs filesystem itself is a
  storage-layout decision left to whoever runs this installer).
- Warns (does not fail) if `/usr/bin/newuidmap`/`newgidmap` are missing or not root-owned-setuid —
  install your distro's `uidmap` package if so.

## Granting workspace lifecycle and scoped host-verification capabilities

`workspace_storage.rs` shells out to `/usr/bin/btrfs` (a hardcoded absolute path — there is
deliberately no override, so the pin can never silently point at a different binary) for
`qgroup limit`, `subvolume delete`, and `sync` (needs `CAP_SYS_ADMIN`), and to `/usr/bin/chown` to
hand a fresh workspace to the job's own userns-mapped uid/gid (needs `CAP_CHOWN` — a distinct
capability from `CAP_SYS_ADMIN`, easy to miss). **Do not** `setcap cap_sys_admin+ep /usr/bin/btrfs`
— that grants the capability to every process on the host that can execute `btrfs`, not just the
runner. Instead, `deploy/systemd/myelin-ci-controlplane.service` grants them ONLY to this service
via:

```ini
AmbientCapabilities=CAP_SYS_ADMIN CAP_CHOWN CAP_DAC_READ_SEARCH
NoNewPrivileges=false
```

Ambient capabilities propagate from the granted process to everything it `exec`s (including its
`btrfs`/`chown` child processes) without either process needing its own file capabilities — this is
the mechanism for `CAP_SYS_ADMIN` and `CAP_CHOWN`, not a workaround. `CAP_DAC_READ_SEARCH` has a
different, narrower lifecycle: the binary lowers it from ambient and clears it from effective on
the initial thread **before constructing Tokio**, retaining it only in permitted. The dedicated
runner thread temporarily makes it effective around the bounded `O_NOFOLLOW` reads of `.git/HEAD`
and `Cargo.lock`, then clears it again. Thus `runsc` and workload processes never inherit it. This
is a read/search-only DAC bypass; **never substitute `CAP_DAC_OVERRIDE`**, which also bypasses write
permission checks. The unavoidable deployment tradeoff is that systemd's trusted `ExecStart` shell
and the binary's few pre-runtime instructions briefly receive the ambient capability; avoiding that
would require a privileged launcher or a file capability on a shared executable, both broader and
harder to audit.

**Do NOT also restrict `CapabilityBoundingSet` to just these capabilities** (an earlier draft of
this unit did, and it was wrong): `runsc`'s explicit
user-namespace mode execs `newuidmap`/`newgidmap` — setuid-root helpers — to write the uid/gid
mapping. A setuid-root exec can never gain a capability outside the CALLING process's bounding set,
so narrowing it breaks that escalation with an opaque `newuidmap failed: fork/exec ... operation not
permitted` (hit and confirmed directly). Leave `CapabilityBoundingSet` at systemd's default (the
full set). `NoNewPrivileges` MUST stay `false` for the same
setuid-helper reason — `NoNewPrivileges=true` (a common systemd hardening default — do not apply it
here) makes the kernel ignore their setuid bit on exec entirely.

## Cgroup v2 delegation for per-job memory and CPU bounding

`MemoryCgroup::create` (`gvisor.rs`) creates each job's memory/CPU-bounding cgroup as a SIBLING of the
runner process's own cgroup — it needs write access to its own cgroup's PARENT directory, with
`memory` and `cpu` already enabled in that parent's `cgroup.subtree_control`. The unit achieves this
with:

```ini
Delegate=memory cpu
DelegateSubgroup=supervisor
ExecStart=/bin/sh -c 'CG="/sys/fs/cgroup$(sed -n "s#^0::##p" /proc/self/cgroup)"; echo +memory +cpu > "$CG/../cgroup.subtree_control"; exec /opt/myelin/bin/ci-controlplane'
```

`DelegateSubgroup=supervisor` places the actual service process one level below the delegated
boundary (in a `supervisor/` subgroup), so the boundary itself — this process's cgroup's parent —
is what's delegated, leaving room for sibling cgroups beside `supervisor/`. `Delegate=memory cpu`
chowns that boundary to the service account but does **not** itself populate its `cgroup.subtree_control`
(confirmed directly: without the explicit write, the boundary is owned by `myelin-runner` yet has an
EMPTY `cgroup.subtree_control`, so `supervisor/cgroup.controllers` lacks `memory` and `cpu` and
`MemoryCgroup::create` fails closed with `Permission denied`).

**The `+memory +cpu` write MUST happen from `ExecStart`, never `ExecStartPre`.** This was confirmed
directly and is not obvious: an `ExecStartPre` doing the exact same write deterministically fails
the ENTIRE unit start with `Failed to spawn executor: Device or resource busy` — a systemd/kernel
cgroup-v2 ordering interaction specific to a freshly delegated unit's own startup sequence (the
delegated boundary's cgroup hierarchy is not yet in the state this write needs while `ExecStartPre`
runs, even a single unrelated `ExecStartPre` step placed immediately before it does not help). The
identical write succeeds every time once moved into `ExecStart` — by which point the process has
already landed in its final `supervisor/` location and the boundary is fully settled. The wrapper
uses `exec` (not a plain subshell) to replace itself with the real binary so systemd's
MainPID/signal tracking is unaffected.

## PostgreSQL definition-fence provisioning

**Required before deploying a binary that carries `ci_0020h`, on every existing database.** A fresh
Docker volume provisions itself (the `pg-init` scripts run in filename order); an existing database
does not, so this is a deliberate two-stage rollout.

The `ci.pipeline` definition cutover drains the superseded workflow definition and activates the new
one in a single transaction, gated on a database-wide backlog probe. That probe must see past
`workflow_run`'s FORCE row-level security — otherwise it returns `false` rather than raising, and the
cutover drains a definition that live runs still depend on. The authority lives in one dedicated
`BYPASSRLS` role, provisioned by an operator rather than by a migration: migration behaviour must not
vary with whatever privileges the migration credential happens to hold.

**Order (do not deploy before step 1 on each database):**

1. Run the provisioning script with a cluster-admin/superuser credential, passing the ACTUAL role
   behind `DATABASE_MIGRATION_URL`:

   ```sh
   psql "$DATABASE_PROVISIONING_URL" \
     --set=ON_ERROR_STOP=1 \
     --set=migration_role=myelin_admin \
     --file scripts/pg-init/01-ci-definition-fence.sql
   ```

2. Verify the postconditions the script prints: the role is
   `NOLOGIN NOSUPERUSER BYPASSRLS NOINHERIT`, `myelin_ci_security` is owned by it, and the migration
   role holds exactly one membership edge with `admin=false, inherit=false, set=true`.
3. Deploy the new binary.
4. Boot applies `ci_0020h` (verifies the provisioning, grants the fence role `SELECT` on exactly
   `wf_type, wf_version, state`, and creates the probe already owned by the fence role) and then
   `ci_0020i` (seeds the cutover's predecessor row).
5. Only then does the definition cutover run, as the last gate before the runner lane starts.

**If step 1 is skipped**, `ci_0020h` fails before it is recorded and before anything is drained. The
error names this script and the exact `migration_role=` argument. Already-deployed binaries are
untouched and keep serving; the failure is safe, loud and actionable. Re-run step 1 and reboot.

The script is idempotent and safe to re-run. It refuses — rather than mass-revoking — if a role of
the same name already exists owning or holding anything outside this dedicated scope, since that
would mean the name was previously used for another purpose.

`scripts/drill-ci-definition-fence-fresh-postgres.sh` proves the fresh-volume half of this against a
throwaway `postgres:16` container, including a non-superuser migration role; the persistent dev stack
cannot cover init-ordering by construction.

## Installing the service

```sh
sudo cp deploy/systemd/myelin-ci-controlplane.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now myelin-ci-controlplane
```

Populate `/etc/myelin/ci-controlplane.env` (root-readable, `0600`) with the rest of
`ci-controlplane`'s required configuration (`DATABASE_URL`, `DATABASE_MIGRATION_URL`,
`MYELIN_CELL_ID`, etc.) — the unit's own `Environment=` lines cover only the gVisor-runner-specific
variables. The checked-in unit represents the `MYELIN_CI_WORKSPACE_MODE=enabled` cutover as the
canonical target posture (explicit-userns + `EphemeralDisk` workspaces) — its
`MYELIN_CI_WORKSPACE_CAPACITY_BYTES` value is the one number in that block you should actually
change per host, sized to the volume `MYELIN_CI_WORKSPACES_DIR` lives on. If you want the rootless
base only, delete or comment out the workspace-activation `Environment=` block (the six lines from
`MYELIN_CI_WORKSPACE_MODE` down) before installing.

## Verifying the host without installing the service

The exact end-to-end proof this doc's provisioning is meant to satisfy is
`crates/myelin-ci-sandbox/src/gvisor.rs`'s
`explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch` test — a live drill that
constructs the real `GvisorBackend::try_new(Enabled)` and calls `.launch()`. It skips gracefully
unless `MYELIN_USERNS_DRILL_LEASES_DIR` is set to a directory satisfying the same strict shape this
script provisions for `userns-leases` (see the test's own doc comment for exactly why it cannot
provision that directory itself).

**This drill needs the same `CAP_SYS_ADMIN`/`CAP_CHOWN`-via-ambient-capabilities plus cgroup
delegation the real service needs — plain `setcap` on the test binary does NOT work.** This was
tried first and confirmed insufficient: `setcap cap_sys_admin+ep <binary>` sets a FILE capability on
that exact binary, and per `capabilities(7)`, executing a binary that has its own file capabilities
CLEARS the process's ambient capability set — so the capability never reaches the `btrfs`/`chown`
child processes the test spawns, even though the test binary's own process appears to have it. The
only mechanism that actually works is the same one the real unit uses: run the drill through a
throwaway transient systemd unit built the same way as
`deploy/systemd/myelin-ci-controlplane.service`, pointed at drill-only leaf directories (never the
real `userns-leases`/`ci-workspaces`/`gvisor-runsc-root` a running service also uses):

```sh
sudo install -d -m 0700 -o "$(whoami)" -g "$(id -gn)" \
  /var/lib/myelin/userns-leases-drill /opt/myelin/gvisor-runsc-root-drill

BIN=$(cargo test -p myelin-ci-sandbox --lib --features integration,test-support --no-run 2>&1 \
  | sed -n 's/^ *Executable unittests.*(\(.*\))$/\1/p')

sudo systemd-run --uid="$(whoami)" --gid="$(id -gn)" --unit=myelin-userns-drill \
  --property=AmbientCapabilities='CAP_SYS_ADMIN CAP_CHOWN CAP_DAC_READ_SEARCH' \
  --property=NoNewPrivileges=false \
  --property='Delegate=memory cpu' \
  --property=DelegateSubgroup=supervisor \
  --property=WorkingDirectory="$(pwd)" \
  --setenv=MYELIN_RUNSC_BIN=/opt/myelin/bin/runsc \
  --setenv=MYELIN_EXPLICIT_USERNS_RUNSC_ROOT=/opt/myelin/gvisor-runsc-root-drill \
  --setenv=MYELIN_USERNS_DRILL_LEASES_DIR=/var/lib/myelin/userns-leases-drill \
  --setenv=HOME="$HOME" \
  --collect \
  -- /bin/sh -c 'CG="/sys/fs/cgroup$(sed -n "s#^0::##p" /proc/self/cgroup)"; echo +memory +cpu > "$CG/../cgroup.subtree_control"; exec '"$BIN"' --test-threads=1 --nocapture explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch'

sudo journalctl -u myelin-userns-drill --no-pager -o cat
```

(The `+memory +cpu` write is inline in the drill's own `ExecStart`-equivalent for the same reason the real
unit's is — see the cgroup-delegation section above; a separate `ExecStartPre`/pre-step
deterministically fails unit startup here too.) A passing run leaves no leaked Btrfs subvolume,
qgroup, or userns lease marker behind — verify with `sudo btrfs qgroup show /`, `runsc list`, and
`ls /var/lib/myelin/userns-leases-drill`, all of which should show nothing new after the test
completes. This exact recipe (through a real `myelin-ci-controlplane.service`-shaped unit, not the
raw test binary) is what confirmed the full activation path — subvolume creation, `qgroup limit`,
`chown`, memory cgroup creation, durable lease bind, real `runsc` container boot under the explicit
mapping, and clean teardown — end to end on a real host.
