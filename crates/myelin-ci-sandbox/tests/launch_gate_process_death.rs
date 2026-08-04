#![cfg(feature = "test-support")]

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn path(label: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "myelin-launch-death-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn sigkill_of_runner_process_kills_released_runtime_before_delayed_effect() {
    let armed = path("armed");
    let escaped = path("escaped");
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("launch_gate_process_helper")
        .arg("--ignored")
        .arg("--nocapture")
        .env("MYELIN_LAUNCH_GATE_HELPER", "1")
        .env("MYELIN_LAUNCH_GATE_ARMED", &armed)
        .env("MYELIN_LAUNCH_GATE_ESCAPED", &escaped)
        .spawn()
        .expect("spawn disposable runner process");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !armed.exists() {
        if let Some(status) = helper.try_wait().unwrap() {
            panic!("runner helper exited before arming the launch guard: {status}");
        }
        assert!(Instant::now() < deadline, "runner helper did not arm");
        std::thread::sleep(Duration::from_millis(10));
    }

    helper.kill().expect("SIGKILL disposable runner process");
    helper.wait().expect("reap disposable runner process");
    std::thread::sleep(Duration::from_millis(1_250));
    assert!(
        !escaped.exists(),
        "sandbox runtime survived the death of its owning runner process"
    );
    let _ = std::fs::remove_file(armed);
    let _ = std::fs::remove_file(escaped);
}

#[test]
fn stopped_runner_cannot_pause_runtime_past_the_independent_deadline() {
    let armed = path("stopped-armed");
    let escaped = path("stopped-escaped");
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("launch_gate_process_helper")
        .arg("--ignored")
        .arg("--nocapture")
        .env("MYELIN_LAUNCH_GATE_HELPER", "1")
        .env("MYELIN_LAUNCH_GATE_ARMED", &armed)
        .env("MYELIN_LAUNCH_GATE_ESCAPED", &escaped)
        .spawn()
        .expect("spawn disposable runner process");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !armed.exists() {
        if let Some(status) = helper.try_wait().unwrap() {
            panic!("runner helper exited before arming the launch guard: {status}");
        }
        assert!(Instant::now() < deadline, "runner helper did not arm");
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        unsafe { libc::kill(helper.id() as i32, libc::SIGSTOP) },
        0,
        "stop disposable runner process"
    );
    std::thread::sleep(Duration::from_millis(1_250));
    assert!(
        !escaped.exists(),
        "runtime survived past its deadline while the owning runner was stopped"
    );
    unsafe {
        libc::kill(helper.id() as i32, libc::SIGCONT);
    }
    helper
        .kill()
        .expect("kill resumed disposable runner process");
    helper.wait().expect("reap disposable runner process");
    let _ = std::fs::remove_file(armed);
    let _ = std::fs::remove_file(escaped);
}

#[test]
#[ignore = "invoked only as the disposable process-death helper"]
fn launch_gate_process_helper() {
    if std::env::var_os("MYELIN_LAUNCH_GATE_HELPER").is_none() {
        return;
    }
    let armed = std::path::PathBuf::from(std::env::var_os("MYELIN_LAUNCH_GATE_ARMED").unwrap());
    let escaped = std::path::PathBuf::from(std::env::var_os("MYELIN_LAUNCH_GATE_ESCAPED").unwrap());
    myelin_ci_sandbox::launch_gate_parent_death_probe(&armed, &escaped).unwrap();
}
