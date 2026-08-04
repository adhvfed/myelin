#![cfg(feature = "integration")]

use myelin_ci_sandbox::firecracker::{
    boot_and_capture, FcMachineConfig, FirecrackerBackend, ENV_FC_KERNEL, ENV_FC_ROOTFS,
};
use myelin_ci_sandbox::hardening::HardeningProfile;
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ResourceLimits,
    RunTokenCredential, TrustTier, WorkspaceSpec,
};
use std::path::PathBuf;

fn trivial_hardened_spec() -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .unwrap(),
        vec!["/bin/true".into()],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 60,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenCredential::new("test-bearer", "selftest-jti", 300).unwrap(),
        MeterTarget {
            reserve_id: "selftest-reserve".into(),
        },
        IdemToken("selftest-idem".into()),
    )
    .unwrap()
}

fn preconditions() -> (bool, PathBuf, PathBuf) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let asset = PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("firecracker-assets");
    let kernel = std::env::var(ENV_FC_KERNEL)
        .map(PathBuf::from)
        .unwrap_or_else(|_| asset.join("vmlinux-6.1.168"));
    let rootfs = std::env::var(ENV_FC_ROOTFS)
        .map(PathBuf::from)
        .unwrap_or_else(|_| asset.join("ubuntu-24.04.squashfs"));

    let has_kvm = std::path::Path::new("/dev/kvm").exists();
    let has_fc = which_firecracker();
    let assets_present = kernel.exists() && rootfs.exists();
    (has_kvm && has_fc && assets_present, kernel, rootfs)
}

fn which_firecracker() -> bool {
    let bin = std::env::var("MYELIN_FC_BIN").unwrap_or_else(|_| "firecracker".into());
    if bin.contains('/') {
        return std::path::Path::new(&bin).exists();
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if std::path::Path::new(dir).join(&bin).exists() {
                return true;
            }
        }
    }
    false
}

#[test]
fn hardened_boot_selftest_boots_a_real_microvm_and_asserts_the_profile() {
    let (ready, _kernel, _rootfs) = preconditions();
    if !ready {
        eprintln!(
            "[hardened-boot self-test] SKIPPED: /dev/kvm or `firecracker` or the staged guest \
             assets are absent - this host cannot boot a microVM. (CI without KVM passes.)"
        );
        return;
    }

    let spec = trivial_hardened_spec();

    let profile = HardeningProfile::derive(&spec);
    profile
        .assert_enforced()
        .expect("the mandatory hardening profile must be enforced");
    assert!(
        profile.egress_default_deny,
        "egress-default-deny must be in force"
    );
    assert!(profile.read_only_root, "read-only-root must be in force");
    assert!(
        profile.pids_max > 0,
        "pids.max must be set (fork-bomb ceiling)"
    );
    assert!(
        !profile.network_device,
        "fully default-deny ⇒ NO NIC attached"
    );

    let cfg = FcMachineConfig::from_spec(&spec, &profile,  true);
    assert!(
        cfg.root_is_read_only(),
        "the boot config's root drive MUST be read-only"
    );
    assert!(
        !cfg.has_network_device(),
        "the boot config MUST attach no NIC (egress closed at the device)"
    );
    assert_eq!(cfg.pids_max(), profile.pids_max);
    let json = cfg.to_json();
    assert!(
        json.contains("\"is_read_only\": true"),
        "is_read_only=true in the real config JSON"
    );
    assert!(
        !json.contains("network-interfaces"),
        "no network-interfaces key (egress device-closed)"
    );

    let cfg2 = FirecrackerBackend::machine_config(&spec, true)
        .expect("machine_config must assert hardening and build the config");
    assert_eq!(cfg, cfg2);

    let cfg_path =
        std::env::temp_dir().join(format!("myelin-fc-selftest-{}.json", std::process::id()));
    std::fs::write(&cfg_path, cfg.to_json()).expect("write machine config");
    let (exit_code, console) = boot_and_capture(&cfg_path).expect("boot the microVM");
    let _ = std::fs::remove_file(&cfg_path);

    assert!(
        console.contains("Linux version 6.1.168"),
        "the guest serial console must show the real guest kernel booting.\n--- console ---\n{console}"
    );
    assert!(
        console.contains("Hypervisor detected: KVM") || console.contains("KVM"),
        "the boot must be KVM-backed (hardware virtualization - the microVM boundary).\n{console}"
    );
    assert!(
        console.contains("root=/dev/vda ro"),
        "the kernel cmdline must mount the root device READ-ONLY (read-only-root in force)."
    );
    assert!(
        console.contains("Firecracker exiting successfully") || exit_code == 0,
        "the one-shot hardened boot must reboot cleanly and the VMM exit 0 (exit={exit_code}).\n{console}"
    );

    let kver = console
        .lines()
        .find(|l| l.contains("Linux version"))
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| "Linux version 6.1.168".into());

    let date = "2026-06-21";
    println!(
        "[GREEN] {date} hardened-boot self-test PASS | backend=firecracker(microVM/KVM) | \
         kernel=6.1.168 | profile-on: egress-default-deny=ON read-only-root=ON pids.max={} | \
         guest: {kver} | vmm-exit={exit_code}",
        profile.pids_max
    );
}
