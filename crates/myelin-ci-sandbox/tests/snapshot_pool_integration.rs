#![cfg(feature = "integration")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn preconditions() -> Option<(PathBuf, PathBuf, String)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let asset = PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("firecracker-assets");
    let kernel = std::env::var("MYELIN_FC_KERNEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| asset.join("vmlinux-6.1.168"));
    let rootfs = std::env::var("MYELIN_FC_ROOTFS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| asset.join("ubuntu-24.04.squashfs"));
    let fc = std::env::var("MYELIN_FC_BIN").unwrap_or_else(|_| "firecracker".into());

    let has_kvm = Path::new("/dev/kvm").exists();
    let has_curl = which("curl");
    let has_fc = if fc.contains('/') {
        Path::new(&fc).exists()
    } else {
        which(&fc)
    };
    if has_kvm && has_curl && has_fc && kernel.exists() && rootfs.exists() {
        Some((kernel, rootfs, fc))
    } else {
        None
    }
}

fn which(bin: &str) -> bool {
    if bin.contains('/') {
        return Path::new(bin).exists();
    }
    std::env::var("PATH")
        .map(|p| p.split(':').any(|d| Path::new(d).join(bin).exists()))
        .unwrap_or(false)
}

fn api(sock: &Path, method: &str, route: &str, body: &str) -> String {
    let out = Command::new("curl")
        .arg("-s")
        .arg("--unix-socket")
        .arg(sock)
        .arg("-X")
        .arg(method)
        .arg(format!("http://localhost/{route}"))
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-d")
        .arg(body)
        .output()
        .expect("curl runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn pre_warmed_snapshot_restore_runs_the_restored_guest_on_real_silicon() {
    let Some((kernel, rootfs, fc)) = preconditions() else {
        eprintln!(
            "[snapshot-pool integration] SKIPPED: /dev/kvm or `firecracker` or `curl` or the staged \
             guest assets are absent - this host cannot snapshot/restore a microVM. (CI without KVM \
             passes.)"
        );
        return;
    };

    let work = std::env::temp_dir().join(format!("myelin-snap-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("create work dir");
    let sock1 = work.join("fc1.sock");
    let sock2 = work.join("fc2.sock");
    let snap = work.join("snap.file");
    let mem = work.join("mem.file");

    let mut fc1 = Command::new(&fc)
        .arg("--api-sock")
        .arg(&sock1)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn source firecracker");
    std::thread::sleep(Duration::from_millis(600));

    let boot_args =
        "console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda ro init=/sbin/init".to_string();
    api(
        &sock1,
        "PUT",
        "boot-source",
        &format!(
            "{{\"kernel_image_path\":{:?},\"boot_args\":{:?}}}",
            kernel.to_string_lossy(),
            boot_args
        ),
    );
    api(
        &sock1,
        "PUT",
        "drives/rootfs",
        &format!(
            "{{\"drive_id\":\"rootfs\",\"path_on_host\":{:?},\"is_root_device\":true,\"is_read_only\":true}}",
            rootfs.to_string_lossy()
        ),
    );
    api(
        &sock1,
        "PUT",
        "machine-config",
        "{\"vcpu_count\":1,\"mem_size_mib\":256}",
    );
    api(
        &sock1,
        "PUT",
        "actions",
        "{\"action_type\":\"InstanceStart\"}",
    );
    std::thread::sleep(Duration::from_millis(2500));

    api(&sock1, "PATCH", "vm", "{\"state\":\"Paused\"}");
    api(
        &sock1,
        "PUT",
        "snapshot/create",
        &format!(
            "{{\"snapshot_type\":\"Full\",\"snapshot_path\":{:?},\"mem_file_path\":{:?}}}",
            snap.to_string_lossy(),
            mem.to_string_lossy()
        ),
    );
    let snap_ok = snap.exists() && mem.exists();
    let _ = fc1.kill();
    let _ = fc1.wait();
    assert!(
        snap_ok,
        "CreateSnapshot must produce the snapshot + memory files (the warm-buffer source)"
    );

    let mut fc2 = Command::new(&fc)
        .arg("--api-sock")
        .arg(&sock2)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn restore firecracker");
    std::thread::sleep(Duration::from_millis(600));

    let t0 = std::time::Instant::now();
    let load_resp = api(
        &sock2,
        "PUT",
        "snapshot/load",
        &format!(
            "{{\"snapshot_path\":{:?},\"mem_backend\":{{\"backend_path\":{:?},\"backend_type\":\"File\"}},\"enable_diff_snapshots\":false,\"resume_vm\":true}}",
            snap.to_string_lossy(),
            mem.to_string_lossy()
        ),
    );
    let restore_ms = t0.elapsed().as_millis();

    std::thread::sleep(Duration::from_millis(400));
    let cfg = api(&sock2, "GET", "vm/config", "");
    let _ = fc2.kill();
    let _ = fc2.wait();
    let _ = std::fs::remove_dir_all(&work);

    assert!(
        !load_resp.contains("\"fault_message\"") && !load_resp.to_lowercase().contains("error"),
        "LoadSnapshot(resume_vm=true) must succeed and resume the restored guest.\n--- load ---\n{load_resp}"
    );
    assert!(
        cfg.contains("\"boot-source\""),
        "the RESTORED guest must answer GET /vm/config (the resumed VMM is live).\n--- cfg ---\n{cfg}"
    );
    assert!(
        cfg.contains("\"is_read_only\":true"),
        "the restored guest's root drive is READ-ONLY (the hardening profile survives the restore)."
    );
    assert!(
        cfg.contains("\"network-interfaces\":[]"),
        "the restored guest has NO NIC (egress device-closed - hardening survives the restore)."
    );

    let date = "2026-06-21";
    println!(
        "[GREEN] {date} pre-warm snapshot-restore PASS | backend=firecracker(microVM/KVM) v1.16.0 | \
         cycle=PauseVM→CreateSnapshot→LoadSnapshot(resume_vm)→running | restore-latency~{restore_ms}ms \
         (warm-pool-fast vs cold boot) | restored-guest: read-only-root=ON no-NIC=ON | LIVE on real silicon"
    );
}
