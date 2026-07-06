# CI (myelin-ci-controlplane, myelin-ci-dispatch, myelin-ci-sandbox)

_I reviewed the three CI crates by mapping their module trees and deep-reading the security-load-bearing modules. In myelin-ci-sandbox I read: lib.rs (JobSpec/ImageRef::digest_pinned, the SandboxBackend trait + RunnerHooks four-guarantee seam), hardening.rs (HardeningProfile::derive/assert_enforced + EgressEvaluator/EgressDecision), firecracker.rs (FcMachineConfig::from_spec/to_json + launch_with + spawn_real_vmm), gvisor.rs (OciConfig + GvisorBackend::launch_with + build_gvisor_corpus_script/gvisor_drill_config_json), self_hosted.rs (AttestState machine, StructuralAttestationVerifier, SelfHostedRunner::may_claim, TenantScopedToken::admits, mint_self_hosted_token), runner.rs (JobLeaseStore::claim_for_labels/heartbeat, EngineTerminalReporter, RunnerAgent::run_one) and snapshot_pool.rs (SnapshotPool::acquire/warm_up, WarmSandbox). In myelin-ci-controlplane I read secret_broker.rs (SecretBroker::resolve fork short-circuit + authz gate, mint_oidc) and fairness.rs (FairShare DRR advance/replenish, Backpressure per-tenant cap, shed_order). In myelin-ci-dispatch I read dispatch.rs (classify_trust/stamp_trust/git_trust_of, DedupLedger) and resolve.rs (resolve_snapshot digest-pin gate, validate_dag, reserve_and_start). Overall the unit is unusually healthy and defense-minded: the tenant-scoping (self-hosted attestation + TenantScopedToken cross-tenant refusal), the fail-closed trust classifier (OR-based fork verdict, single stamp with 0 divergence), the fork-gets-no-secrets structural short-circuit in SecretBroker::resolve, the exactly-once job.done via engine ON CONFLICT, the DRR fairness/backpressure DoS controls, and the one-job-per-sandbox pooling invariant are all correct and well-tested. The main real defect is that the egress-allowlist enforcement is never wired into the datapath: EgressEvaluator (the component that enforces "metadata/control-plane/cross-tenant ALWAYS blocked" and allowlist-only egress) is only ever invoked from its own unit tests, while the backends carry only a boolean has_network_device into the guest config, so any job that opts into egress gets an unfiltered NIC. Two smaller robustness gaps: ImageRef::digest_pinned accepts any-length hex (no digest-length/algo validation), and the gVisor escape-drill OCI bundle uses a no-op seccomp default action while production uses errno._

**Kept findings:** 3  (🟠 1 high  ·  🔵 2 low)

---

### 1. 🟠 Egress allowlist / metadata-SSRF block is computed but never enforced in the datapath

- **Severity:** high  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** security
- **Location:** `crates/myelin-ci-sandbox/src/firecracker.rs:137`

**What:** Verified in source. HardeningProfile::derive (hardening.rs:194-210) collapses the egress allowlist to a single bool network_device = !allowlist.is_empty(); that bool is all FcMachineConfig::from_spec (firecracker.rs:108-131) and OciConfig::from_spec (gvisor.rs:65-75) carry. When the allowlist is non-empty, FcMachineConfig::to_json (firecracker.rs:137-140) attaches a NIC on a hardcoded host tap 'tap-myelin' with NO per-destination filter derived from the allowlist; gVisor adds a full network namespace (gvisor.rs:86-91). A grep across all three CI crates confirms EgressEvaluator::evaluate/is_allowed are called only from (a) hardening.rs::assert_enforced (which merely rejects an allowlist that literally NAMES a blocked host) and (b) unit tests — never to emit host firewall/nftables rules. No code in these crates produces the tap-device filter the firecracker.rs:138 comment ('the host wires the egress allowlist via the tap device's firewall') claims. So the documented always-blocked classes (169.254.169.254, RFC-1918, cross-tenant) and the allowlist itself are dropped from the runtime datapath once any egress is requested.

**Impact:** For any job opting into egress (non-empty allow list), the guest receives an effectively unfiltered NIC. The advertised metadata-SSRF/cross-tenant/control-plane block (AG-D4) is not enforced at runtime by any code in these crates; assert_enforced only rejects a literally-named blocked host, not the guest reaching those hosts by IP/DNS at runtime. Enforcement depends entirely on out-of-crate host tap firewall wiring that is asserted in a comment but never emitted.

**Fix:** Carry HardeningProfile.egress_allowlist (not just the bool) into the backend configs; generate concrete per-guest host firewall rules (default-deny + resolved allowlist IPs + explicit drops for 169.254/8, RFC-1918, loopback) via EgressEvaluator; give each guest a unique tap device instead of the constant 'tap-myelin'; resolve DNS at rule-build time; treat a non-empty allowlist as fail-closed until the datapath filter exists.

> _Verifier note:_ Confirmed by reading firecracker.rs:99-160 and 271-300 (launch path uses from_spec+to_json; no filter emission), hardening.rs:194-258 (derive collapses to bool; assert_enforced only string-matches allowlist entries), gvisor.rs:65-106, and a repo-wide grep showing evaluate/is_allowed have no production caller beyond assert_enforced. The hardcoded 'tap-myelin' at firecracker.rs:140 confirms the shared-tap concern. Severity high is well-calibrated given the module advertises an ALWAYS-blocked hard guarantee; nuance: the tap-firewall wiring is arguably intended as host-provisioning outside these crates, but nothing in-crate emits or fail-closes on its absence, so the runtime guarantee is unmet as shipped.

### 2. 🔵 ImageRef::digest_pinned accepts any-length hex digest (weak supply-chain pin)

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** security
- **Location:** `crates/myelin-ci-sandbox/src/lib.rs:222`

**What:** Verified at lib.rs:222-230. digest_pinned() returns true when the reference contains '@<algo>:<hex>' with a non-empty algo and a non-empty all-hex digest: `!algo.is_empty() && !digest.is_empty() && digest.chars().all(|c| c.is_ascii_hexdigit())`. No length check per algorithm and no known-algo allowlist, so 'img@sha256:a' and 'img@bogus:ff' both pass. resolve.rs:401 (myelin-ci-dispatch) uses this exact predicate as the fail-closed digest-pin gate (`if !image.digest_pinned()`).

**Impact:** A truncated or bogus 'digest' passes the plan-time gate that is supposed to guarantee reproducible, tamper-evident pinning. The advertised invariant ('0 un-digested references reach a snapshot') is weaker than stated at this layer; real integrity still depends on downstream registry resolution rejecting it.

**Fix:** Validate digest length per algorithm (sha256 -> 64 hex, sha512 -> 128) and restrict <algo> to a known set; reject anything else as UndigestedImage. Small self-contained change in ImageRef::digest_pinned.

> _Verifier note:_ Confirmed by reading lib.rs:203-231 (the predicate) and grepping myelin-ci-dispatch/src/resolve.rs (line 401 is the sole gate caller). Severity low is correct — defence-in-depth weakening, not an active bypass, since downstream registry resolution is a second line.

### 3. 🔵 gVisor escape-drill OCI bundle uses a no-op seccomp default action

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `crates/myelin-ci-sandbox/src/gvisor.rs:380`

**What:** Verified. gvisor_drill_config_json emits `"seccomp": { "defaultAction": "SCMP_ACT_ALLOW" }` (gvisor.rs:380) for the CI-P28 escape-drill bundle, whereas production OciConfig::to_json emits `"seccomp": { "defaultAction": "SCMP_ACT_ERRNO" }` (gvisor.rs:98). The drill therefore runs the adversarial corpus under an allow-all seccomp default action, a materially weaker posture than production's deny-by-default.

**Impact:** The gVisor escape-drill attestation ('contained on the second backend') does not faithfully exercise production's seccomp filter; a containment regression that seccomp would catch could still pass the drill. Not an active escape — corpus containment also relies on caps-drop/no-net/ro-root/no-CAP_MKNOD — but the attestation is less meaningful than claimed.

**Fix:** Make the drill bundle mirror the production default action (SCMP_ACT_ERRNO), or document explicitly why the drill relaxes it.

> _Verifier note:_ Confirmed by direct comparison of gvisor.rs:98 (production, SCMP_ACT_ERRNO) vs gvisor.rs:380 (drill, SCMP_ACT_ALLOW). The module docstring (gvisor.rs:251-254) claims the drill bundle expresses the SAME mandatory posture, which the seccomp default action contradicts. Severity low is appropriate.
