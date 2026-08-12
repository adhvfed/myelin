use super::git_wire_codec::pkt_line_encode;
use super::*;
use crate::{ImageRef, JobSpec, LaunchPermit};
#[cfg(feature = "test-support")]
use crate::{ResourceUsage, SandboxResult};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub(crate) fn sha1_oid(byte: u8) -> String {
    format!("{:02x}", byte).repeat(20)
}

pub(crate) fn advertisement(first_line: &str, extra_refs: &[&str]) -> Vec<u8> {
    let mut buf = pkt_line_encode(first_line);
    for line in extra_refs {
        buf.extend(pkt_line_encode(line));
    }
    buf.extend_from_slice(b"0000");
    buf
}

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

pub(crate) fn fake_pack(payload: &[u8]) -> Vec<u8> {
    let mut pack = b"PACK".to_vec();
    pack.extend_from_slice(payload);
    pack
}

pub(crate) fn advertisement_bytes(oid: &str) -> Vec<u8> {
    advertisement(
        &format!("{oid} refs/heads/main\0no-progress ofs-delta shallow\n"),
        &[],
    )
}

pub(crate) fn fetch_response_bytes(payload: &[u8]) -> Vec<u8> {
    fetch_response(&[], "NAK", &fake_pack(payload))
}

pub(crate) fn fake_quiescence_evidence() -> RuntimeQuiescenceEvidence {
    RuntimeQuiescenceEvidence::assert_for_tests(
        "5b3-3-test-container".to_string(),
        RuntimeNamespaceQuiescence::Rootless,
        CgroupQuiescenceEvidence::assert_for_tests((1, 1)),
    )
}

pub(crate) type ScriptedStep = Box<dyn FnOnce() -> Result<(ContainerRun, bool), RunFailure> + Send>;

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

pub(crate) struct FakeRunsc;
impl RunscChild for FakeRunsc {
    fn kill(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        Ok(0)
    }
}

#[cfg(feature = "test-support")]
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
pub(crate) fn checkout_commit_oid() -> String {
    sha1_oid(0xC7)
}

pub fn stage_checkout_repo_root(root: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(
        root.join(CHECKOUT_TENANT)
            .join(CHECKOUT_REGION)
            .join(format!("{CHECKOUT_REPO}.git")),
    )
    .expect("stage bare repo root");
    root.to_path_buf()
}

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
