use std::process::Command;

const CHILD_ENV: &str = "MYELIN_PANIC_HOOK_CHILD";
const SENTINEL: &str = "PANIC_LEAK_SENTINEL_5c0b697a";

/// Run the hook in a child test process so changing the process-global hook cannot interfere with
/// other tests and the parent can inspect the real OS-level stderr stream.
#[test]
fn payload_free_hook_omits_secret_panic_material_from_stderr() {
    if std::env::var_os(CHILD_ENV).is_some() {
        myelin_events::install_payload_free_panic_hook("panic-hook-regression");
        let panic = std::panic::catch_unwind(|| panic!("credential={SENTINEL}"));
        assert!(
            panic.is_err(),
            "the regression must exercise a caught panic"
        );
        return;
    }

    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "payload_free_hook_omits_secret_panic_material_from_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("spawn isolated panic-hook test process");

    assert!(
        output.status.success(),
        "isolated panic-hook process failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).expect("test stderr must be UTF-8");
    assert!(
        stderr.contains("panic-hook-regression: an internal task panicked; payload suppressed"),
        "the payload-free diagnostic must remain observable: {stderr:?}"
    );
    assert!(
        !stderr.contains(SENTINEL),
        "panic payload leaked to process stderr: {stderr:?}"
    );
}
