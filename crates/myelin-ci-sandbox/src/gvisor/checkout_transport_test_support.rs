//! CT-007 slice 5b.3-6e.2 Stage A: the git-wire test-support substrate (fakes + wire fixtures).

use super::git_wire_codec::pkt_line_encode;
use super::*;
use crate::{ImageRef, JobSpec, LaunchPermit, ResourceUsage, SandboxResult};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// A 40-hex SHA-1-shaped commit oid built from a single repeated byte.
pub(crate) fn sha1_oid(byte: u8) -> String {
    format!("{:02x}", byte).repeat(20)
}

/// Assemble a pkt-line `upload-pack` advertisement (first ref line + optional extra refs + flush).
pub(crate) fn advertisement(first_line: &str, extra_refs: &[&str]) -> Vec<u8> {
    let mut buf = pkt_line_encode(first_line);
    for line in extra_refs {
        buf.extend(pkt_line_encode(line));
    }
    buf.extend_from_slice(b"0000");
    buf
}

/// Assemble a fetch response: optional shallow lines, a flush, a negotiation line, then the pack.
pub(crate) fn fetch_response(shallow_lines: &[String], negotiation: &str, pack: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    for line in shallow_lines {
        buf.extend(pkt_line_encode(line));
    }
    buf.extend_from_slice(b"0000");
    buf.extend(pkt_line_encode(negotiation));
    buf.extend_from_slice(pack);
    buf
}

/// A minimal well-formed pack: the `PACK` magic followed by an arbitrary payload.
pub(crate) fn fake_pack(payload: &[u8]) -> Vec<u8> {
    let mut pack = b"PACK".to_vec();
    pack.extend_from_slice(payload);
    pack
}

/// A directly-advertised advertisement for `oid` with the required capabilities.
pub(crate) fn advertisement_bytes(oid: &str) -> Vec<u8> {
    advertisement(
        &format!("{oid} refs/heads/main\0no-progress ofs-delta shallow\n"),
        &[],
    )
}

/// A NAK fetch response carrying a `PACK`-wrapped payload, no shallow line.
pub(crate) fn fetch_response_bytes(payload: &[u8]) -> Vec<u8> {
    fetch_response(&[], "NAK", &fake_pack(payload))
}

/// A canned, always-fine teardown proof for the auto-wrapped `Finalized` case —
/// `RuntimeNamespaceQuiescence::Rootless` since git-wire's `prepared_mode` is always `Rootless`.
pub(crate) fn fake_quiescence_evidence() -> RuntimeQuiescenceEvidence {
    RuntimeQuiescenceEvidence::assert_for_tests(
        "5b3-3-test-container".to_string(),
        RuntimeNamespaceQuiescence::Rootless,
        CgroupQuiescenceEvidence::assert_for_tests((1, 1)),
    )
}

/// A single scripted Hop-A step: the simple pre-finalization outcome the recording executor
/// auto-wraps into a `RuntimeFinalization::Finalized`.
pub(crate) type ScriptedStep = Box<dyn FnOnce() -> Result<(ContainerRun, bool), RunFailure> + Send>;

/// A boxed stand-in for [`GitWireHopExecutor`] — call sites pass `&*executor`.
pub(crate) type BoxedHopExecutor = Box<
    dyn Fn(
        &JobSpec,
        &OciConfig,
        Vec<u8>,
        &Path,
        &AtomicBool,
        LaunchPermit,
    ) -> (
        Result<GitWireHopFinalization, RunFailure>,
        BundleCleanupProof,
    ),
>;

/// A deterministic stand-in for the spawned `runsc` child: `kill`/`wait` are clean no-op successes.
pub(crate) struct FakeRunsc;
impl RunscChild for FakeRunsc {
    fn kill(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        Ok(0)
    }
}

/// A canned [`ContainerRun`] carrying `stdout`/`usage` — a clean exit-0 result, a fake child, and a
/// REAL (freshly-staged, unique) bundle dir so the git-wire hop's CHECKED post-run bundle removal
/// proves clean (a non-existent dir would make the checked teardown fail → `TeardownUnproven`). The
/// test-support analog of `checkout_transport_5b3_3::fake_hop_container_run`.
pub(crate) fn fake_git_wire_run(stdout: Vec<u8>, usage: ResourceUsage) -> ContainerRun {
    let bundle_dir = std::env::temp_dir().join(format!(
        "myelin-git-wire-test-support-bundle-{}-{}",
        std::process::id(),
        unique_suffix(),
    ));
    std::fs::create_dir_all(&bundle_dir).expect("stage a real bundle dir for the git-wire hop");
    ContainerRun {
        child: Box::new(FakeRunsc),
        bundle_dir,
        result: SandboxResult {
            exit_code: Some(0),
            timed_out: false,
            usage,
            stdout,
            stderr: Vec::new(),
        },
        run_error: None,
    }
}

/// An executor that COMMITS every permit it is handed and records `(run-token JTI, committed)` per
/// call — so a test can prove each leg spawned under its OWN credential and its OWN durable phase
/// permit. Scripts exactly `steps.len()` calls; panics if invoked more times than scripted.
#[allow(clippy::type_complexity)]
pub(crate) fn permit_recording_executor(
    steps: Vec<ScriptedStep>,
) -> (BoxedHopExecutor, Arc<Mutex<Vec<(String, bool)>>>) {
    let seen: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_closure = Arc::clone(&seen);
    let queue = Mutex::new(std::collections::VecDeque::from(steps));
    let f = move |job: &JobSpec,
                  _cfg: &OciConfig,
                  _stdin: Vec<u8>,
                  _rootfs: &Path,
                  _cancellation: &AtomicBool,
                  permit: LaunchPermit| {
        let committed = permit.commit_and_release().is_ok();
        seen_for_closure
            .lock()
            .unwrap()
            .push((job.run_token.jti.clone(), committed));
        let mut queue = queue.lock().unwrap();
        let step = queue
            .pop_front()
            .expect("executor invoked more times than scripted");
        let finalization_result = match step() {
            Ok((run, truncated)) => Ok(RuntimeFinalization::Finalized(FinalizedRun {
                primary: Ok((run, truncated)),
                evidence: fake_quiescence_evidence(),
            })),
            Err(run_failure) => Err(run_failure),
        };
        (finalization_result, Ok(()))
    };
    (Box::new(f), seen)
}

pub(crate) const CHECKOUT_TENANT: &str = "acme";
pub(crate) const CHECKOUT_REGION: &str = "fr-par";
pub(crate) const CHECKOUT_REPO: &str = "widgets";
/// The exact 40-hex commit the [`checkout_spec_for_backend`] job advertises — the driver scripts an
/// advertisement/fetch for THIS oid, and the orchestrator derives its `ExpectedGitCommitId` from it.
pub(crate) fn checkout_commit_oid() -> String {
    sha1_oid(0xC7)
}

/// Stage the bare-repo directory both Hop A resolutions require: `root/<tenant>/<region>/<repo>.git`,
/// a REAL directory (never a symlink), matching `resolve_bare_repo_path`/`assert_repo_under_root`.
pub fn stage_checkout_repo_root(root: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(
        root.join(CHECKOUT_TENANT)
            .join(CHECKOUT_REGION)
            .join(format!("{CHECKOUT_REPO}.git")),
    )
    .expect("stage bare repo root");
    root.to_path_buf()
}

/// Build a deterministic **Enabled** [`GvisorBackend`] under `root` — a real-digest registry binding
/// (a staged empty rootfs hashed with the SAME `canonical_tree_sha256_hex` the registry uses, so the
/// pin genuinely resolves) plus a deterministic-directory workspace manager and a fixture-`subuid`
/// userns allocator. Struct-literal construction: this submodule is a descendant of the `gvisor`
/// module, so it may name `GvisorBackend`'s private fields. Returns the backend + the resolvable
/// [`ImageRef`] a matching checkout spec must carry. No Btrfs / `/etc/subuid` / KVM / runsc.
pub fn deterministic_enabled_backend_for_tests(root: &Path) -> (GvisorBackend, ImageRef) {
    let rootfs = root.join("rootfs");
    std::fs::create_dir_all(&rootfs).expect("stage the workload rootfs dir");
    std::fs::create_dir(rootfs.join("workspace"))
        .expect("precreate the pinned workspace mountpoint");
    let digest =
        crate::canonical_tar::canonical_tree_sha256_hex(&rootfs).expect("hash the staged rootfs");
    let image = ImageRef::pinned(format!("test.local/checkout-workload@sha256:{digest}")).unwrap();
    let registry = Arc::new(
        crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![
            crate::asset_registry::RootfsAssetBinding {
                image: image.clone(),
                rootfs,
            },
        ])
        .expect("the real-digest fixture binding verifies"),
    );
    let workspace_base = root.join("workspace");
    let userns_base = root.join("userns");
    std::fs::create_dir_all(&workspace_base).unwrap();
    std::fs::create_dir_all(&userns_base).unwrap();
    let workspace_manager = deterministic_workspace_manager_for_tests(workspace_base, 1 << 30)
        .expect("deterministic-directory workspace manager must construct");
    let userns_allocator = deterministic_userns_allocator_for_tests(&userns_base, 1)
        .expect("deterministic userns allocator must construct (a NON-root user is required)");
    let backend = GvisorBackend {
        live: Mutex::new(std::collections::HashMap::new()),
        registry: Some(registry),
        workspace_integration: WorkspaceIntegration::Enabled {
            workspace_manager,
            userns_allocator,
        },
        checkout: GvisorCheckoutConfig::disabled(),
        rootfs_overlay: None,
    };
    (backend, image)
}

/// A checkout-bearing CI [`JobSpec`] for the deterministic Enabled backend: `image` is the backend's
/// resolvable rootfs pin, the workspace carries the repo ref + [`checkout_commit_oid`], and the
/// run-token authorization is a `CiJob` context whose `region` the orchestrator threads into the Hop
/// A bare-repo path.
pub fn checkout_spec_for_backend(image: ImageRef) -> crate::JobSpec {
    let mut spec = crate::JobSpec::new(
        crate::JobKind::Ci,
        image,
        vec!["true".into()],
        vec![],
        vec![],
        crate::EgressPolicy { allow: vec![] },
        crate::ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 64,
            timeout_secs: 120,
        },
        crate::WorkspaceSpec {
            repo_ref: Some(format!(
                "myelin://{CHECKOUT_TENANT}/git/repo/{CHECKOUT_REPO}"
            )),
            commit: Some(checkout_commit_oid()),
        },
        crate::TrustTier::UntrustedFork,
        crate::RunTokenCredential::new("test-bearer", "j-checkout", 300).unwrap(),
        crate::MeterTarget {
            reserve_id: "r".into(),
        },
        crate::IdemToken("idem-6e2-checkout".into()),
    )
    .unwrap();
    spec.run_token_authorization = Some(crate::RunTokenAuthorizationContext::CiJob(
        crate::CiJobAuthorizationContext {
            tenant_id: CHECKOUT_TENANT.to_string(),
            region: CHECKOUT_REGION.to_string(),
            principal_id: "p".to_string(),
            project_id: "00000000-0000-0000-0000-000000000001".to_string(),
            wf_run_id: "wf".to_string(),
            job_id: "j".to_string(),
            lease_owner: "o".to_string(),
            lease_epoch: 1,
            claim_nonce: "n".to_string(),
            claim_started_at_epoch_secs: 0,
            claim_expires_at_epoch_secs: 1,
            reserve_id: "r".to_string(),
            required_capabilities: vec![],
            checkout_scope: None,
            credential_binding: None,
        },
    ));
    spec
}
