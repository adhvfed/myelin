# CI runner (gVisor sandbox) host deployment

The CI control plane's runner lane (`ci-controlplane` with `MYELIN_CI_RUNNER=1`) launches untrusted
job commands inside a real `runsc` (gVisor) sandbox — the same no-host-exec seam the escape-drill
corpus (CT-003/AG-D4) proves closed. This doc covers ONLY the CI-sandbox/gVisor-specific host
requirements, following the same path conventions as `docs/edge-deployment.md`
(`/opt/myelin/...` for pinned binaries/immutable assets, `/var/lib/myelin/...` for mutable persistent
state); the rest of `ci-controlplane`'s configuration (PostgreSQL roles, cell id, etc.) is out of
scope here.

## Two activation levels

**Rootless (works today).** The runner boots every guest with `runsc --rootless`, no host
capabilities beyond an ordinary unprivileged process. This is what `MYELIN_CI_RUNNER=1` activates as
of this writing — `crates/myelin-ci-controlplane/src/runner_bind.rs`'s `GvisorBackend::new(...)`
call constructs a `Disabled`-workspace-integration backend.

**Explicit user-namespace + `EphemeralDisk` workspaces (CT-007 slice 4 — host prerequisites ready,
code wiring still open).** Each job gets its own real subordinate uid/gid mapping (`UserNamespaceLease`)
and a real Btrfs-quota'd scratch subvolume (`ManagedWorkspace`) instead of an unbounded host-backed
`/tmp`. The HOST-side provisioning this needs (directories, the pinned `runsc` binary's placement,
the `CAP_SYS_ADMIN`/`CAP_CHOWN` grants, and cgroup delegation) is what this doc and
`scripts/install-ci-runner-host.sh` set up. The
CODE-side wiring — `runner_bind.rs` constructing `GvisorBackend::try_new(.., GvisorWorkspaceConfig::
Enabled { .. }, ..)` instead of `::new(..)`, and `main.rs`'s `preflight_runner_host()` additionally
calling `preflight_explicit_userns_policy` — has not landed yet (see
`planning/system-reviews/2026-06-26/12-ci-track-ledger.md`'s slice-3 entry, "still open: slice 4").
Provisioning the host now means that code change is the ONLY remaining step once it's written.

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

## Granting `CAP_SYS_ADMIN`/`CAP_CHOWN` without widening the host's trust surface

`workspace_storage.rs` shells out to `/usr/bin/btrfs` (a hardcoded absolute path — there is
deliberately no override, so the pin can never silently point at a different binary) for
`qgroup limit`, `subvolume delete`, and `sync` (needs `CAP_SYS_ADMIN`), and to `/usr/bin/chown` to
hand a fresh workspace to the job's own userns-mapped uid/gid (needs `CAP_CHOWN` — a distinct
capability from `CAP_SYS_ADMIN`, easy to miss). **Do not** `setcap cap_sys_admin+ep /usr/bin/btrfs`
— that grants the capability to every process on the host that can execute `btrfs`, not just the
runner. Instead, `deploy/systemd/myelin-ci-controlplane.service` grants both ONLY to this service
via:

```ini
AmbientCapabilities=CAP_SYS_ADMIN CAP_CHOWN
NoNewPrivileges=false
```

Ambient capabilities propagate from the granted process to everything it `exec`s (including its
`btrfs`/`chown` child processes) without either process needing its own file capabilities — this is
the mechanism, not a workaround. **Do NOT also restrict `CapabilityBoundingSet` to just these two
capabilities** (an earlier draft of this unit did, and it was wrong): `runsc`'s explicit
user-namespace mode execs `newuidmap`/`newgidmap` — setuid-root helpers — to write the uid/gid
mapping. A setuid-root exec can never gain a capability outside the CALLING process's bounding set,
so narrowing it breaks that escalation with an opaque `newuidmap failed: fork/exec ... operation not
permitted` (hit and confirmed directly). Leave `CapabilityBoundingSet` at systemd's default (the
full set); `AmbientCapabilities` alone already keeps what this process itself actively wields to
exactly `CAP_SYS_ADMIN`/`CAP_CHOWN`. `NoNewPrivileges` MUST stay `false` for the same
setuid-helper reason — `NoNewPrivileges=true` (a common systemd hardening default — do not apply it
here) makes the kernel ignore their setuid bit on exec entirely.

## Cgroup v2 delegation for per-job memory bounding

`MemoryCgroup::create` (`gvisor.rs`) creates each job's memory-bounding cgroup as a SIBLING of the
runner process's own cgroup — it needs write access to its own cgroup's PARENT directory, with
`memory` already enabled in that parent's `cgroup.subtree_control`. The unit achieves this with:

```ini
Delegate=memory
DelegateSubgroup=supervisor
ExecStart=/bin/sh -c 'CG="/sys/fs/cgroup$(sed -n "s#^0::##p" /proc/self/cgroup)"; echo +memory > "$CG/../cgroup.subtree_control"; exec /opt/myelin/bin/ci-controlplane'
```

`DelegateSubgroup=supervisor` places the actual service process one level below the delegated
boundary (in a `supervisor/` subgroup), so the boundary itself — this process's cgroup's parent —
is what's delegated, leaving room for sibling cgroups beside `supervisor/`. `Delegate=memory` chowns
that boundary to the service account but does **not** itself populate its `cgroup.subtree_control`
(confirmed directly: without the explicit write, the boundary is owned by `myelin-runner` yet has an
EMPTY `cgroup.subtree_control`, so `supervisor/cgroup.controllers` shows no `memory` and
`MemoryCgroup::create` fails closed with `Permission denied`).

**The `+memory` write MUST happen from `ExecStart`, never `ExecStartPre`.** This was confirmed
directly and is not obvious: an `ExecStartPre` doing the exact same write deterministically fails
the ENTIRE unit start with `Failed to spawn executor: Device or resource busy` — a systemd/kernel
cgroup-v2 ordering interaction specific to a freshly delegated unit's own startup sequence (the
delegated boundary's cgroup hierarchy is not yet in the state this write needs while `ExecStartPre`
runs, even a single unrelated `ExecStartPre` step placed immediately before it does not help). The
identical write succeeds every time once moved into `ExecStart` — by which point the process has
already landed in its final `supervisor/` location and the boundary is fully settled. The wrapper
uses `exec` (not a plain subshell) to replace itself with the real binary so systemd's
MainPID/signal tracking is unaffected.

## Installing the service

```sh
sudo cp deploy/systemd/myelin-ci-controlplane.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now myelin-ci-controlplane
```

Populate `/etc/myelin/ci-controlplane.env` (root-readable, `0600`) with the rest of
`ci-controlplane`'s required configuration (`DATABASE_URL`, `DATABASE_MIGRATION_URL`,
`MYELIN_CELL_ID`, etc.) — the unit's own `Environment=` lines cover only the gVisor-runner-specific
variables. The explicit-userns/`EphemeralDisk` variables in the unit are commented out and
documented as inert until the `runner_bind.rs`/`main.rs` wiring above lands; uncomment them at that
point, not before (there is nothing to read them yet).

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
  --property=AmbientCapabilities='CAP_SYS_ADMIN CAP_CHOWN' \
  --property=NoNewPrivileges=false \
  --property=Delegate=memory \
  --property=DelegateSubgroup=supervisor \
  --property=WorkingDirectory="$(pwd)" \
  --setenv=MYELIN_RUNSC_BIN=/opt/myelin/bin/runsc \
  --setenv=MYELIN_EXPLICIT_USERNS_RUNSC_ROOT=/opt/myelin/gvisor-runsc-root-drill \
  --setenv=MYELIN_USERNS_DRILL_LEASES_DIR=/var/lib/myelin/userns-leases-drill \
  --setenv=HOME="$HOME" \
  --collect \
  -- /bin/sh -c 'CG="/sys/fs/cgroup$(sed -n "s#^0::##p" /proc/self/cgroup)"; echo +memory > "$CG/../cgroup.subtree_control"; exec '"$BIN"' --test-threads=1 --nocapture explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch'

sudo journalctl -u myelin-userns-drill --no-pager -o cat
```

(The `+memory` write is inline in the drill's own `ExecStart`-equivalent for the same reason the real
unit's is — see the cgroup-delegation section above; a separate `ExecStartPre`/pre-step
deterministically fails unit startup here too.) A passing run leaves no leaked Btrfs subvolume,
qgroup, or userns lease marker behind — verify with `sudo btrfs qgroup show /`, `runsc list`, and
`ls /var/lib/myelin/userns-leases-drill`, all of which should show nothing new after the test
completes. This exact recipe (through a real `myelin-ci-controlplane.service`-shaped unit, not the
raw test binary) is what confirmed the full activation path — subvolume creation, `qgroup limit`,
`chown`, memory cgroup creation, durable lease bind, real `runsc` container boot under the explicit
mapping, and clean teardown — end to end on a real host.
