use super::*;
use crate::hardening::HardeningProfile;
use crate::JobSpec;
use std::path::{Path, PathBuf};

pub const ENV_GVISOR_ROOTFS: &str = "MYELIN_GVISOR_ROOTFS";

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

pub const LINUX_SMALL_V1_ROOTFS_SHA256: &str =
    "65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

pub const LINUX_RUST_V1_ROOTFS_SHA256: &str =
    "e6684d70e026a1433a7e32e2d29c100468d08579ef532834fdd27d4808c35a60";

pub const GVISOR_GIT_ROOTFS_SHA256: &str =
    "0ac70764ba20a043d19933213d60070c7f8712947a86753bab518569df302646";

pub const ENV_GVISOR_RUST_ROOTFS: &str = "MYELIN_GVISOR_RUST_ROOTFS";

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

pub const GVISOR_CORPUS_SCRIPT: &str = "myelin-agd4-corpus.sh";

pub fn build_gvisor_corpus_script(pids_max: u32) -> String {
    use crate::escape_corpus::{BEGIN_MARKER, CORPUS_VERSION, END_MARKER};
    format!(
        r#"#!/bin/sh
echo "{begin} corpus_version={cv} kernel=$(uname -r 2>/dev/null) guest_euid=$(id -u 2>/dev/null)"

if modprobe dummy 2>/dev/null || insmod /x.ko 2>/dev/null; then echo "K1_module ESCAPED"; else echo "K1_module CONTAINED"; fi
if [ -e /dev/mem ]; then echo "K2_devmem ESCAPED present"; elif mknod /dev/mem c 1 1 2>/dev/null; then echo "K2_devmem ESCAPED mknod"; else echo "K2_devmem CONTAINED"; fi
if [ -e /dev/port ]; then echo "K3_ioport ESCAPED present"; elif mknod /dev/port c 1 4 2>/dev/null; then echo "K3_ioport ESCAPED mknod"; else echo "K3_ioport CONTAINED"; fi
if mount -t tmpfs none /mnt 2>/dev/null; then echo "K4_mount ESCAPED"; else echo "K4_mount CONTAINED"; fi

if timeout 3 wget -q -T 2 -O- http://169.254.169.254/latest/meta-data/ 2>/dev/null; then echo "M1_metadata_ssrf ESCAPED"; else echo "M1_metadata_ssrf CONTAINED"; fi
if timeout 3 nc -w 2 10.0.0.1 8080 </dev/null 2>/dev/null; then echo "C1_controlplane ESCAPED"; else echo "C1_controlplane CONTAINED"; fi
if timeout 3 nc -w 2 192.168.13.37 443 </dev/null 2>/dev/null; then echo "T1_crosstenant ESCAPED"; else echo "T1_crosstenant CONTAINED"; fi
SECRET="myelin-drill-canary-$$"
if timeout 3 nc -w 2 93.184.216.34 80 </dev/null 2>/dev/null; then echo "E1_secret_exfil ESCAPED $SECRET"; else echo "E1_secret_exfil CONTAINED"; fi

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

if echo x 2>/dev/null > /root_write_probe; then echo "D1_root_readonly ESCAPED"; rm -f /root_write_probe 2>/dev/null; else echo "D1_root_readonly CONTAINED"; fi
mkdir -p /tmp/scratch 2>/dev/null
if dd if=/dev/zero of=/tmp/scratch/fill bs=1M count=512 2>/dev/null; then echo "D2_diskfill ESCAPED"; else echo "D2_diskfill CONTAINED"; fi

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

pub fn gvisor_drill_config_json(spec: &JobSpec, script_name: &str) -> Result<String, GvisorError> {
    let profile = HardeningProfile::derive(spec);
    profile.assert_enforced().map_err(GvisorError::Hardening)?;
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

pub const ENV_GVISOR_GIT_ROOTFS: &str = "MYELIN_GVISOR_GIT_ROOTFS";

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
            "staged gVisor git rootfs has DRIFTED - expected canonical-tree \
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
        assert!(script.contains("mknod /dev/mem"));
        assert!(script.contains("mknod /dev/port"));
        assert!(script.contains("ceiling=64"));
        assert!(script.contains("trap cleanup_f1 EXIT"));
        assert!(script.contains("exit 42"));
        assert!(script.contains("[ \"$admitted\" -gt 64 ]"));
        assert!(script.contains("F1_forkbomb ESCAPED admitted=$admitted"));
        let reap = script.find("cleanup_f1()").unwrap();
        let verdict = script.find("if [ \"$f1_status\" -eq 42 ]").unwrap();
        let diskfill = script.find("if dd if=/dev/zero").unwrap();
        assert!(
            script.contains("wait 2>/dev/null || true") && reap < verdict && verdict < diskfill
        );
        let attempt = script
            .find(&format!("{} ATTEMPT", crate::escape_corpus::MEMHOG_ID))
            .expect("memhog ATTEMPT sentinel in the gVisor corpus");
        let end = script.find(crate::escape_corpus::END_MARKER).unwrap();
        assert!(
            attempt < end,
            "the memhog ATTEMPT sentinel must precede the END marker"
        );
        assert!(script.contains(r#"S="$S$S""#) && script.contains("while [ $n -lt 26 ]"));
    }

    #[test]
    fn gvisor_drill_config_expresses_the_mandatory_posture() {
        let json = gvisor_drill_config_json(&spec(vec![]), GVISOR_CORPUS_SCRIPT).unwrap();
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"bounding\": []"));
        assert!(json.contains("\"limit\": 64"));
        assert!(json.contains("\"type\": \"RLIMIT_NPROC\""));
        assert!(
            !json.contains("\"type\": \"network\""),
            "no network namespace ⇒ egress closed (--network=none leaves only loopback)"
        );
        assert!(
            !json.contains("\"type\": \"user\""),
            "the rootless gofer fork fails with a doubly-declared user namespace"
        );
        assert!(json.contains(GVISOR_CORPUS_SCRIPT));
    }
}
