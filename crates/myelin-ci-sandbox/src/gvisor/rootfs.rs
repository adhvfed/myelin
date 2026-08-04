//! Resolving + verifying the staged gVisor rootfs assets (escape-drill, Rust, and git-bearing), plus
//! the escape-drill corpus script and bundle config the CI-P28 gate runs.

use super::*;
use crate::hardening::HardeningProfile;
use crate::JobSpec;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------------------------
// CI-P28 (P-423) — the escape drill RE-RUNS on the gVisor backend (the permanent gate, 8.4).
//
// The Firecracker drill (CI-P5) boots a microVM and runs the corpus as PID1; gVisor's drill runs
// the SAME seven adversarial families inside a real `runsc` (gVisor) userspace-kernel sandbox via a
// minimal OCI bundle (`runsc run --bundle`). The bundle expresses the SAME mandatory hardening
// posture the [`HardeningProfile`] computes (read-only root, all caps dropped, no-new-privs, pids
// ceiling, NO network namespace), so the corpus is contained by the SAME profile, enforced through
// gVisor's mechanism (the OCI spec) instead of Firecracker's (the microVM drive/NIC config).
//
// BACKEND-SHAPED PROBE (an honest, documented deviation — EI-01 §1). The corpus tests a *property*
// (no raw physical-memory / I/O-port access); the in-guest *probe* of that property is necessarily
// backend-shaped. On Firecracker the privileged device nodes are real and EPERM on write; on gVisor
// they are simply ABSENT and creating one is denied (no CAP_MKNOD). So the gVisor corpus probes
// `K2_devmem`/`K3_ioport` as "the node is absent AND mknod is denied" rather than "dd EPERMs" — the
// SAME contained property, the faithful gVisor expression of it. The marker ids + the host-side
// parser ([`parse_console`]) are IDENTICAL across backends, so the gate predicate is one path.
// ---------------------------------------------------------------------------------------------

/// Env var naming the staged minimal OCI rootfs the gVisor escape drill runs the corpus in (a clean
/// rootfs with NO privileged device nodes — busybox-class). Defaults to the staged asset dir.
pub const ENV_GVISOR_ROOTFS: &str = "MYELIN_GVISOR_ROOTFS";

/// The resolved minimal rootfs path for the gVisor drill (env override →
/// `~/.local/share/gvisor-assets/rootfs`). The drill SKIPS gracefully if it is absent.
pub fn resolved_gvisor_rootfs() -> std::path::PathBuf {
    if let Ok(p) = std::env::var(ENV_GVISOR_ROOTFS) {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    std::path::PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("rootfs")
}

/// The canonical-tree sha256 of the STAGED base/`linux-small-v1` rootfs [`resolved_gvisor_rootfs`]
/// resolves by default — the SAME digest `.myelin/ci.toml` already pins for the founder-dogfood
/// pipeline's `myelin.local/linux-small-v1-rootfs` image (`scripts/dogfood.sh`'s `verify_ci_rootfs`
/// asserts this on that ONE file; kept here as a single Rust-side source of truth for composition
/// roots/tests that need to build a real [`crate::asset_registry::GvisorAssetRegistry`] entry for
/// it, rather than re-typing the hex string at every call site).
pub const LINUX_SMALL_V1_ROOTFS_SHA256: &str =
    "65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

/// The canonical-tree sha256 of the STAGED `linux-rust-v1` rootfs [`resolved_gvisor_rust_rootfs`]
/// resolves by default — the SAME digest committed in `runner-assets.toml`'s `linux-rust-v1` row.
pub const LINUX_RUST_V1_ROOTFS_SHA256: &str =
    "e6684d70e026a1433a7e32e2d29c100468d08579ef532834fdd27d4808c35a60";

/// Canonical-tree digest of the staged git-bearing rootfs. Unlike the former env-path-only
/// checkout authority, this pin covers the complete tree including every fixed OCI mount target.
pub const GVISOR_GIT_ROOTFS_SHA256: &str =
    "0ac70764ba20a043d19933213d60070c7f8712947a86753bab518569df302646";

/// Env var naming the staged Rust-capable gVisor rootfs (mirrors `runner-assets.toml`'s
/// `linux-rust-v1` row: `env_var = "MYELIN_GVISOR_RUST_ROOTFS"`).
pub const ENV_GVISOR_RUST_ROOTFS: &str = "MYELIN_GVISOR_RUST_ROOTFS";

/// The resolved Rust-capable rootfs path (env override → `~/.local/share/gvisor-assets/rust-rootfs`,
/// `runner-assets.toml`'s `linux-rust-v1` row `default_path`). SEPARATE from
/// [`resolved_gvisor_rootfs`] because this asset carries a real Rust toolchain the plain
/// busybox-class base rootfs does not; nothing dispatches jobs against it by default today — only
/// the registry entry the CT-007 gate-2 composition root registers, and the rust-capability
/// prod-exec self-test.
pub fn resolved_gvisor_rust_rootfs() -> std::path::PathBuf {
    if let Ok(p) = std::env::var(ENV_GVISOR_RUST_ROOTFS) {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    std::path::PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("rust-rootfs")
}

/// The name of the in-guest corpus script the OCI bundle runs (placed at the rootfs root). It runs
/// as the container's `process.args` entrypoint and prints the SAME `<id> CONTAINED|ESCAPED` markers
/// the Firecracker corpus does, bracketed by the SAME begin/end markers, so [`parse_console`] over
/// the captured gVisor console produces a [`DrillReport`](crate::escape_corpus::DrillReport) exactly
/// as for Firecracker (ONE gate predicate, two backends).
pub const GVISOR_CORPUS_SCRIPT: &str = "myelin-agd4-corpus.sh";

/// Build the in-guest corpus script for the gVisor backend — the SAME seven adversarial families as
/// [`build_corpus_script`](crate::escape_corpus::build_corpus_script), expressed for gVisor's device
/// model (mknod-denial for the raw-device family; `--network=none` ⇒ no route for the egress family;
/// the OCI `pids.limit` for the fork bomb; read-only root for disk fill). `pids_max` is the OCI
/// `linux.resources.pids.limit` the bundle sets. The markers + ids are byte-identical to the
/// Firecracker corpus so the host-side parser is one path.
pub fn build_gvisor_corpus_script(pids_max: u32) -> String {
    use crate::escape_corpus::{BEGIN_MARKER, CORPUS_VERSION, END_MARKER};
    // `pids_max` is a u32 decimal literal — no shell-injection surface.
    format!(
        r#"#!/bin/sh
# AG-D4 / CI-T1 adversarial escape corpus (corpus_version={cv}) — gVisor (runsc) backend.
# Run inside a real runsc userspace-kernel sandbox via a hardened OCI bundle. Markers parsed by the
# SAME host-side parser as the Firecracker drill: "<id> CONTAINED" iff the attack genuinely failed.
echo "{begin} corpus_version={cv} kernel=$(uname -r 2>/dev/null) guest_euid=$(id -u 2>/dev/null)"

# ---- kernel-exploit primitives (run UNDER the bundle's all-caps-dropped + no-new-privs posture) ----
# K1: load a kernel module — gVisor has no module support; EPERM/unsupported ⇒ contained.
if modprobe dummy 2>/dev/null || insmod /x.ko 2>/dev/null; then echo "K1_module ESCAPED"; else echo "K1_module CONTAINED"; fi
# K2: raw physical-memory device — ABSENT in the hardened rootfs AND mknod is denied (no CAP_MKNOD).
if [ -e /dev/mem ]; then echo "K2_devmem ESCAPED present"; elif mknod /dev/mem c 1 1 2>/dev/null; then echo "K2_devmem ESCAPED mknod"; else echo "K2_devmem CONTAINED"; fi
# K3: raw I/O-port device — same (absent + mknod denied).
if [ -e /dev/port ]; then echo "K3_ioport ESCAPED present"; elif mknod /dev/port c 1 4 2>/dev/null; then echo "K3_ioport ESCAPED mknod"; else echo "K3_ioport CONTAINED"; fi
# K4: privileged mount — denied (no CAP_SYS_ADMIN).
if mount -t tmpfs none /mnt 2>/dev/null; then echo "K4_mount ESCAPED"; else echo "K4_mount CONTAINED"; fi

# ---- egress family: the OCI bundle has NO network namespace (--network=none) ⇒ only loopback,
#      no route to any external/metadata/control-plane/cross-tenant host. ----
if timeout 3 wget -q -T 2 -O- http://169.254.169.254/latest/meta-data/ 2>/dev/null; then echo "M1_metadata_ssrf ESCAPED"; else echo "M1_metadata_ssrf CONTAINED"; fi
if timeout 3 nc -w 2 10.0.0.1 8080 </dev/null 2>/dev/null; then echo "C1_controlplane ESCAPED"; else echo "C1_controlplane CONTAINED"; fi
if timeout 3 nc -w 2 192.168.13.37 443 </dev/null 2>/dev/null; then echo "T1_crosstenant ESCAPED"; else echo "T1_crosstenant CONTAINED"; fi
SECRET="myelin-drill-canary-$$"
if timeout 3 nc -w 2 93.184.216.34 80 </dev/null 2>/dev/null; then echo "E1_secret_exfil ESCAPED $SECRET"; else echo "E1_secret_exfil CONTAINED"; fi

# ---- fork bomb: the OCI bundle sets both linux.resources.pids.limit and RLIMIT_NPROC={pids_max}.
#      Run the saturation loop in a child shell: BusyBox exits that child with status 2 when the
#      rlimit rejects fork, while the corpus PID 1 survives to reap children and continue D1/D2/Mx. ----
F1_COUNT=/tmp/myelin-f1-count
: > "$F1_COUNT"
(
  children=""
  cleanup_f1() {{ for child in $children; do kill "$child" 2>/dev/null || true; done; wait 2>/dev/null || true; }}
  trap cleanup_f1 EXIT
  n=0; admitted=0
  while [ $n -lt 300 ]; do
    /bin/sleep 30 &
    child=$!
    children="$children $child"
    admitted=$((admitted+1))
    echo "$admitted" > "$F1_COUNT"
    n=$((n+1))
  done
  if [ $admitted -le {pids_max} ]; then exit 0; else exit 42; fi
) 2>/dev/null
f1_status=$?
admitted=0
[ -r "$F1_COUNT" ] && read admitted < "$F1_COUNT"
rm -f "$F1_COUNT" 2>/dev/null || true
if [ "$f1_status" -eq 42 ] || [ "$admitted" -gt {pids_max} ]; then echo "F1_forkbomb ESCAPED admitted=$admitted ceiling={pids_max} status=$f1_status"; elif [ "$f1_status" -eq 0 ] || [ "$f1_status" -eq 2 ]; then echo "F1_forkbomb CONTAINED survived=yes admitted=$admitted ceiling={pids_max} status=$f1_status"; else echo "F1_forkbomb ESCAPED unexpected_probe_status=$f1_status admitted=$admitted ceiling={pids_max}"; fi

# ---- disk fill + read-only root ----
if echo x 2>/dev/null > /root_write_probe; then echo "D1_root_readonly ESCAPED"; rm -f /root_write_probe 2>/dev/null; else echo "D1_root_readonly CONTAINED"; fi
mkdir -p /tmp/scratch 2>/dev/null
if dd if=/dev/zero of=/tmp/scratch/fill bs=1M count=512 2>/dev/null; then echo "D2_diskfill ESCAPED"; else echo "D2_diskfill CONTAINED"; fi

# ---- anonymous-memory hog (Mx_memhog) — bounded by the OUT-OF-BAND host memory cgroup the
#      production run path places the runsc process tree into (rootless runsc does NOT enforce the
#      OCI memory.limit, so the cgroup is the real enforcer). An oversized anon alloc breaches
#      memory.max and the kernel OOM-kills the sentry within the limit ⇒ the WHOLE container dies
#      mid-alloc. So containment is STRUCTURAL: the ATTEMPT sentinel + the END marker are printed
#      (and flushed) BEFORE the alloc; the ESCAPED line prints ONLY if the oversized alloc HELD (the
#      bound failed / the cgroup was dropped). The host-side parser reads ATTEMPT-present-and-ESCAPED-
#      absent as CONTAINED. A regression dropping the cgroup => the hog HELDs => ESCAPED => RED. ----
echo "{memhog_id} ATTEMPT bytes={hog}"
echo "{end}"
( S=aaaaaaaaaaaaaaaa; n=0; while [ $n -lt 26 ]; do S="$S$S"; n=$((n+1)); done; echo "{memhog_id} ESCAPED held=${{#S}}" ) 2>/dev/null
echo "{memhog_id} CONTAINED via_cgroup"
"#,
        cv = CORPUS_VERSION,
        begin = BEGIN_MARKER,
        end = END_MARKER,
        pids_max = pids_max,
        memhog_id = crate::escape_corpus::MEMHOG_ID,
        hog = crate::escape_corpus::MEMHOG_BYTES,
    )
}

/// Build the hardened OCI `config.json` for the gVisor escape-drill bundle from a [`JobSpec`]'s
/// derived [`HardeningProfile`]. It expresses the SAME mandatory posture the Firecracker backend
/// enforces, through the OCI spec gVisor consumes: **read-only root**, **all caps dropped**,
/// **no-new-privileges**, the **pids ceiling**, and **no network namespace** (so `--network=none`
/// leaves only loopback). The entrypoint runs the in-guest corpus script (placed at `/{script}`).
///
/// NOTE: the `user` namespace is deliberately NOT listed — `runsc --rootless` adds its own user
/// namespace, and a doubly-declared userns makes the rootless gofer fork/exec fail. This is the one
/// rootless-specific deviation; the security posture (caps/nnp/ro-root/no-net/pids) is unchanged.
pub fn gvisor_drill_config_json(spec: &JobSpec, script_name: &str) -> Result<String, GvisorError> {
    let profile = HardeningProfile::derive(spec);
    profile.assert_enforced().map_err(GvisorError::Hardening)?;
    // No network namespace ⇒ with `runsc --network=none` only loopback exists (egress closed).
    let json = format!(
        r#"{{
  "ociVersion": "1.0.0",
  "process": {{
    "terminal": false,
    "user": {{ "uid": 0, "gid": 0 }},
    "args": ["/bin/sh", "/{script}"],
    "env": ["PATH=/bin:/sbin:/usr/bin:/usr/sbin"],
    "cwd": "/",
    "noNewPrivileges": {nnp},
    "rlimits": [{{ "type": "RLIMIT_NPROC", "hard": {pids}, "soft": {pids} }}],
    "capabilities": {{ "bounding": [], "effective": [], "permitted": [], "inheritable": [], "ambient": [] }}
  }},
  "root": {{ "path": "rootfs", "readonly": {ro} }},
  "hostname": "myelin-agd4",
  "mounts": [
    {{ "destination": "/proc", "type": "proc", "source": "proc" }},
    {{ "destination": "/dev", "type": "tmpfs", "source": "tmpfs", "options": ["nosuid", "strictatime", "mode=755", "size=65536k"] }},
    {{ "destination": "/tmp", "type": "tmpfs", "source": "tmpfs", "options": ["nosuid", "nodev", "size=8m"] }}
  ],
  "linux": {{
    "namespaces": [
      {{ "type": "pid" }}, {{ "type": "ipc" }}, {{ "type": "uts" }}, {{ "type": "mount" }}
    ],
    "resources": {{ "pids": {{ "limit": {pids} }} }},
    "seccomp": {{ "defaultAction": "SCMP_ACT_ALLOW" }}
  }}
}}"#,
        script = script_name,
        nnp = profile.no_new_privileges,
        ro = profile.read_only_root,
        pids = profile.pids_max,
    );
    Ok(json)
}

/// Env var naming a staged rootfs that CONTAINS `git` (busybox-class rootfs + `git` + its `git-core`
/// helpers + the shared-lib closure). Defaults to `~/.local/share/gvisor-assets/git-rootfs`. The git
/// wire REQUIRES a real `git` in the guest — see the staging recipe in `tests/git_wire_prod_exec_test.rs`.
pub const ENV_GVISOR_GIT_ROOTFS: &str = "MYELIN_GVISOR_GIT_ROOTFS";

/// The resolved rootfs the git-wire container runs in (env override → the staged git-rootfs asset).
/// SEPARATE from [`resolved_gvisor_rootfs`] because the escape-drill rootfs is busybox-only (no `git`);
/// the git wire needs a `git`-bearing rootfs. The launch fails closed (honest `Runtime` error) if it
/// is absent — it never fabricates a result.
pub fn resolved_gvisor_git_rootfs() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_GVISOR_GIT_ROOTFS) {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("git-rootfs")
}

/// Resolve and enforce the git rootfs's content-addressed integrity before it can be used. This is
/// intentionally the same canonical-tree SHA-256 mechanism as [`GvisorAssetRegistry`], plus the
/// git layouts' complete fixed mountpoint contract. Production also registers this exact path and
/// pin at runner construction; the per-exec verification here additionally covers git-wire-only
/// users and detects drift between startup and a later checkout.
pub fn verified_gvisor_git_rootfs() -> Result<PathBuf, String> {
    verify_gvisor_git_rootfs_given(&resolved_gvisor_git_rootfs(), GVISOR_GIT_ROOTFS_SHA256)
}

fn verify_gvisor_git_rootfs_given(
    configured: &Path,
    expected_digest: &str,
) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(configured).map_err(|error| {
        format!(
            "staged gVisor git rootfs {} is absent or invalid: {error}",
            configured.display()
        )
    })?;
    if canonical == Path::new("/") || !canonical.is_dir() {
        return Err(format!(
            "staged gVisor git rootfs {} must resolve to a real, non-root directory",
            canonical.display()
        ));
    }

    // Complete destination enumeration across both layouts using this tree:
    //   checkout: /tmp (tmpfs), /workspace (rw bind)
    //   git wire: /tmp (tmpfs), /repo (ro bind), /quarantine (optional rw bind)
    for destination in ["tmp", "workspace", "repo", "quarantine"] {
        let path = canonical.join(destination);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "git rootfs OCI mount target /{destination} must be precreated in the pinned tree: \
                 {error}"
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "git rootfs OCI mount target /{destination} must be a real directory"
            ));
        }
        let mut entries = std::fs::read_dir(&path)
            .map_err(|error| format!("read git rootfs OCI mount target /{destination}: {error}"))?;
        if entries.next().is_some() {
            return Err(format!(
                "git rootfs OCI mount target /{destination} must be empty before hashing and use"
            ));
        }
    }

    let actual = crate::canonical_tar::canonical_tree_sha256_hex(&canonical)
        .map_err(|error| format!("hash staged gVisor git rootfs: {error}"))?;
    if actual != expected_digest {
        return Err(format!(
            "staged gVisor git rootfs has DRIFTED — expected canonical-tree \
             sha256:{expected_digest}, computed sha256:{actual}"
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::gvisor::test_fixtures::*;

    #[test]
    fn git_rootfs_requires_every_fixed_mountpoint_and_verifies_a_stable_digest() {
        let rootfs =
            std::env::temp_dir().join(format!("myelin-git-rootfs-integrity-{}", unique_suffix()));
        std::fs::create_dir_all(&rootfs).unwrap();
        for destination in ["tmp", "workspace", "repo", "quarantine"] {
            std::fs::create_dir(rootfs.join(destination)).unwrap();
        }
        std::fs::write(rootfs.join("git-payload"), b"pinned git rootfs bytes").unwrap();
        let digest = crate::canonical_tar::canonical_tree_sha256_hex(&rootfs).unwrap();

        let first = verify_gvisor_git_rootfs_given(&rootfs, &digest).unwrap();
        let second = verify_gvisor_git_rootfs_given(&rootfs, &digest).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            crate::canonical_tar::canonical_tree_sha256_hex(&rootfs).unwrap(),
            digest,
            "verification itself must not mutate the shared git rootfs"
        );

        for destination in ["tmp", "workspace", "repo", "quarantine"] {
            std::fs::remove_dir(rootfs.join(destination)).unwrap();
            let error = verify_gvisor_git_rootfs_given(&rootfs, &digest)
                .expect_err("every OCI destination must pre-exist before hashing/use");
            assert!(
                error.contains(&format!("/{destination}")),
                "missing destination must be named: {error}"
            );
            std::fs::create_dir(rootfs.join(destination)).unwrap();
        }

        std::fs::write(rootfs.join("unapproved-drift"), b"drift").unwrap();
        assert!(
            verify_gvisor_git_rootfs_given(&rootfs, &digest)
                .unwrap_err()
                .contains("DRIFTED"),
            "content added outside the mountpoints must fail the pin"
        );
        let _ = std::fs::remove_dir_all(rootfs);
    }

    #[test]
    fn gvisor_corpus_script_carries_every_catalogued_attack_and_the_posture() {
        // The gVisor corpus probes the SAME catalogued attack ids the host-side parser keys on, so
        // the gate predicate is one path across backends. A drift here would let a family silently
        // not run on the gVisor backend.
        let script = build_gvisor_corpus_script(64);
        for atk in crate::escape_corpus::CORPUS {
            assert!(
                script.contains(atk.id),
                "catalogued attack `{}` is missing from the gVisor corpus script",
                atk.id
            );
        }
        assert!(script.contains(crate::escape_corpus::BEGIN_MARKER));
        assert!(script.contains(crate::escape_corpus::END_MARKER));
        // The raw-device family is probed as mknod-denial (the faithful gVisor expression of the
        // contained property — the node is absent and creating one is denied).
        assert!(script.contains("mknod /dev/mem"));
        assert!(script.contains("mknod /dev/port"));
        // The fork-bomb ceiling is carried from the arg.
        assert!(script.contains("ceiling=64"));
        assert!(script.contains("trap cleanup_f1 EXIT"));
        assert!(script.contains("exit 42"));
        assert!(script.contains("[ \"$admitted\" -gt 64 ]"));
        assert!(script.contains("F1_forkbomb ESCAPED admitted=$admitted"));
        // The admitted children are reaped before D2/Mx so the fork-bomb probe cannot consume the
        // shared memory cgroup and vacuously kill a later independent resource probe.
        let reap = script.find("cleanup_f1()").unwrap();
        let verdict = script.find("if [ \"$f1_status\" -eq 42 ]").unwrap();
        let diskfill = script.find("if dd if=/dev/zero").unwrap();
        assert!(
            script.contains("wait 2>/dev/null || true") && reap < verdict && verdict < diskfill
        );
        // CT-003b: the anon-memory hog's ATTEMPT sentinel + the END marker precede the oversized
        // alloc, so the corpus COMPLETES even when the contained hog OOM-kills the whole sentry
        // mid-alloc (the host cgroup bounds host RAM). The ESCAPED line follows only if it HELD.
        let attempt = script
            .find(&format!("{} ATTEMPT", crate::escape_corpus::MEMHOG_ID))
            .expect("memhog ATTEMPT sentinel in the gVisor corpus");
        let end = script.find(crate::escape_corpus::END_MARKER).unwrap();
        assert!(
            attempt < end,
            "the memhog ATTEMPT sentinel must precede the END marker"
        );
        // Pure-shell doubling allocator (holds the anon memory in the sh process itself; ~1 GiB) —
        // the host cgroup OOM-kills the sentry when it breaches memory.max, never a false held=0.
        assert!(script.contains(r#"S="$S$S""#) && script.contains("while [ $n -lt 26 ]"));
    }

    #[test]
    fn gvisor_drill_config_expresses_the_mandatory_posture() {
        let json = gvisor_drill_config_json(&spec(vec![]), GVISOR_CORPUS_SCRIPT).unwrap();
        // Read-only root, no-new-privs, all caps dropped, the pids ceiling — the SAME mandatory
        // profile the Firecracker backend enforces, expressed through the OCI spec gVisor consumes.
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"bounding\": []"));
        assert!(json.contains("\"limit\": 64"));
        assert!(json.contains("\"type\": \"RLIMIT_NPROC\""));
        // NO network namespace ⇒ with --network=none only loopback exists (egress closed).
        assert!(
            !json.contains("\"type\": \"network\""),
            "no network namespace ⇒ egress closed (--network=none leaves only loopback)"
        );
        // The rootless deviation: NO user namespace (runsc --rootless adds its own).
        assert!(
            !json.contains("\"type\": \"user\""),
            "the rootless gofer fork fails with a doubly-declared user namespace"
        );
        // The entrypoint runs the corpus script.
        assert!(json.contains(GVISOR_CORPUS_SCRIPT));
    }
}
