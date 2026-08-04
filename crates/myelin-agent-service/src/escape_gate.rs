use myelin_ci_sandbox::{Backend, EscapeAttestation};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionBackendId {
    pub backend: Backend,
    pub rootfs_sha256: String,
    pub kernel_sha256: String,
    pub corpus_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateRefusal {
    NoAttestation,
    WrongArtifact {
        found_artifact: String,
        found_drill: String,
    },
    Escapes {
        total_escapes: u32,
    },
    GateBackendMismatch {
        attested: Backend,
        expected: Backend,
    },
    ProductionBackendNotExercised {
        backend: Backend,
    },
    IdentityMismatch {
        detail: String,
    },
}

impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateRefusal::NoAttestation => write!(
                f,
                "AG-D4 / CI-T1 GATE: no green escape attestation for the production backend - \
                 REFUSING all untrusted compute (no green attestation ⇒ no untrusted compute; \
                 the gate is fail-closed, agent-fabric.md §2.2 guarantee 4)"
            ),
            GateRefusal::WrongArtifact { found_artifact, found_drill } => write!(
                f,
                "AG-D4 / CI-T1 GATE: attestation is not the green-escape artifact \
                 (artifact=`{found_artifact}` drill=`{found_drill}`) - REFUSING untrusted compute"
            ),
            GateRefusal::Escapes { total_escapes } => write!(
                f,
                "AG-D4 / CI-T1 GATE: a RED attestation with {total_escapes} escape(s) - REFUSING ALL \
                 untrusted compute (one escape is catastrophic; a red AG-D4 blocks all of M3+, \
                 EI-04 §5). ZERO escapes is both the floor and the full answer."
            ),
            GateRefusal::GateBackendMismatch { attested, expected } => write!(
                f,
                "AG-D4 / CI-T1 GATE: the attestation proves backend `{}`, not the production backend \
                 `{}` - REFUSING untrusted compute (the gate is admitted only on the production \
                 backend)",
                attested.key(),
                expected.key()
            ),
            GateRefusal::ProductionBackendNotExercised { backend } => write!(
                f,
                "AG-D4 / CI-T1 GATE: the production backend `{}` was NOT exercised on real silicon in \
                 this drill (recorded as a residual) - REFUSING untrusted compute (a deferred backend \
                 never admits production compute)",
                backend.key()
            ),
            GateRefusal::IdentityMismatch { detail } => write!(
                f,
                "AG-D4 / CI-T1 GATE: the attestation's image/kernel/corpus identity does not match \
                 the production backend ({detail}) - REFUSING untrusted compute (the drill must be \
                 RE-RUN for the new identity; the gate is permanent, re-run on every \
                 backend/image/kernel change)"
            ),
        }
    }
}

impl std::error::Error for GateRefusal {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentExecGate {
    backend_id: ProductionBackendId,
    attested_date: String,
    kernel_version: String,
}

impl AgentExecGate {
    pub fn admit(
        attestation: Option<&EscapeAttestation>,
        backend_id: &ProductionBackendId,
    ) -> Result<AgentExecGate, GateRefusal> {
        let att = attestation.ok_or(GateRefusal::NoAttestation)?;

        if att.artifact != "ag-d4-green-escape-attestation" || att.drill != "AG-D4 / CI-T1" {
            return Err(GateRefusal::WrongArtifact {
                found_artifact: att.artifact.clone(),
                found_drill: att.drill.clone(),
            });
        }

        if att.total_escapes != 0 {
            return Err(GateRefusal::Escapes {
                total_escapes: att.total_escapes,
            });
        }

        if att.gate_backend != backend_id.backend {
            return Err(GateRefusal::GateBackendMismatch {
                attested: att.gate_backend,
                expected: backend_id.backend,
            });
        }

        let exercised = att
            .backends
            .iter()
            .any(|b| b.backend == backend_id.backend && b.exercised);
        if !exercised {
            return Err(GateRefusal::ProductionBackendNotExercised {
                backend: backend_id.backend,
            });
        }

        if att.rootfs_sha256 != backend_id.rootfs_sha256 {
            return Err(GateRefusal::IdentityMismatch {
                detail: format!(
                    "rootfs sha256 attested `{:.16}…` != production `{:.16}…`",
                    att.rootfs_sha256, backend_id.rootfs_sha256
                ),
            });
        }
        if att.kernel_sha256 != backend_id.kernel_sha256 {
            return Err(GateRefusal::IdentityMismatch {
                detail: format!(
                    "kernel sha256 attested `{:.16}…` != production `{:.16}…`",
                    att.kernel_sha256, backend_id.kernel_sha256
                ),
            });
        }
        if att.corpus_version != backend_id.corpus_version {
            return Err(GateRefusal::IdentityMismatch {
                detail: format!(
                    "corpus version attested {} != production {}",
                    att.corpus_version, backend_id.corpus_version
                ),
            });
        }

        Ok(AgentExecGate {
            backend_id: backend_id.clone(),
            attested_date: att.date.clone(),
            kernel_version: att.kernel_version.clone(),
        })
    }

    pub fn admit_from_json(
        json: &str,
        backend_id: &ProductionBackendId,
    ) -> Result<AgentExecGate, GateRefusal> {
        match serde_json::from_str::<EscapeAttestation>(json) {
            Ok(att) => AgentExecGate::admit(Some(&att), backend_id),
            Err(_) => Err(GateRefusal::NoAttestation),
        }
    }

    pub fn backend_id(&self) -> &ProductionBackendId {
        &self.backend_id
    }

    pub fn open_line(&self) -> String {
        format!(
            "[AG-D4 GATE OPEN] date={} gate-backend={} kernel={} corpus-version={} \
             rootfs-sha256={:.16}… - untrusted compute admitted (ZERO escapes attested)",
            self.attested_date,
            self.backend_id.backend.key(),
            self.kernel_version,
            self.backend_id.corpus_version,
            self.backend_id.rootfs_sha256,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
    use myelin_ci_sandbox::{parse_console, BackendRun, CORPUS, CORPUS_VERSION};

    fn prod_id() -> ProductionBackendId {
        ProductionBackendId {
            backend: Backend::FirecrackerMicrovm,
            rootfs_sha256: "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923"
                .to_string(),
            kernel_sha256: "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb"
                .to_string(),
            corpus_version: CORPUS_VERSION,
        }
    }

    fn green_attestation() -> EscapeAttestation {
        let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
        for atk in CORPUS {
            console.push_str(&format!("{} CONTAINED\n", atk.id));
        }
        console.push_str(&format!("{END_MARKER}\n"));
        let report = parse_console(&console);
        let id = prod_id();
        EscapeAttestation::from_green_drill(
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
                    residual_note: Some("runsc residual (CI-P28)".into()),
                },
            ],
            Backend::FirecrackerMicrovm,
            id.rootfs_sha256,
            id.kernel_sha256,
            "6.1.168",
        )
        .expect("the green report mints a green attestation")
    }

    #[test]
    fn no_attestation_refuses_all_untrusted_compute() {
        let r = AgentExecGate::admit(None, &prod_id());
        assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
    }

    #[test]
    fn a_red_attestation_can_never_be_minted_and_so_can_never_admit() {
        let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
        for atk in CORPUS {
            console.push_str(&format!("{} CONTAINED\n", atk.id));
        }
        console = console.replace("K1_module CONTAINED", "K1_module ESCAPED");
        console.push_str(&format!("{END_MARKER}\n"));
        let red = parse_console(&console);
        let minted = EscapeAttestation::from_green_drill(
            "2026-06-21",
            &red,
            vec![BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            }],
            Backend::FirecrackerMicrovm,
            "r",
            "k",
            "6.1.168",
        );
        assert!(minted.is_err(), "a red drill mints NO attestation");
        let r = AgentExecGate::admit(None, &prod_id());
        assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
    }

    #[test]
    fn a_hand_forged_red_escape_count_is_refused() {
        let mut att = green_attestation();
        att.total_escapes = 1;
        let r = AgentExecGate::admit(Some(&att), &prod_id());
        assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 1 });
    }

    #[test]
    fn a_green_matching_attestation_admits_untrusted_compute() {
        let att = green_attestation();
        let gate = AgentExecGate::admit(Some(&att), &prod_id())
            .expect("a green, identity-matched attestation admits");
        assert_eq!(gate.backend_id().backend, Backend::FirecrackerMicrovm);
        assert!(gate.open_line().starts_with("[AG-D4 GATE OPEN]"));
        assert!(gate.open_line().contains("ZERO escapes"));
    }

    #[test]
    fn admit_from_the_real_json_artifact_form() {
        let json = green_attestation().to_json();
        let gate = AgentExecGate::admit_from_json(&json, &prod_id())
            .expect("the green JSON artifact admits");
        assert_eq!(gate.backend_id().corpus_version, CORPUS_VERSION);
    }

    #[test]
    fn an_unparseable_artifact_is_fail_closed() {
        let r = AgentExecGate::admit_from_json("{ not json", &prod_id());
        assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
    }

    #[test]
    fn a_wrong_artifact_kind_never_admits() {
        let mut att = green_attestation();
        att.artifact = "some-other-artifact".into();
        let r = AgentExecGate::admit(Some(&att), &prod_id());
        assert!(matches!(r.unwrap_err(), GateRefusal::WrongArtifact { .. }));
    }

    #[test]
    fn a_gvisor_only_attestation_does_not_admit_the_firecracker_production_backend() {
        let mut att = green_attestation();
        att.gate_backend = Backend::GvisorRunsc;
        let r = AgentExecGate::admit(Some(&att), &prod_id());
        assert_eq!(
            r.unwrap_err(),
            GateRefusal::GateBackendMismatch {
                attested: Backend::GvisorRunsc,
                expected: Backend::FirecrackerMicrovm,
            }
        );
    }

    #[test]
    fn a_production_backend_recorded_as_a_residual_does_not_admit() {
        let mut att = green_attestation();
        for b in att.backends.iter_mut() {
            if b.backend == Backend::FirecrackerMicrovm {
                b.exercised = false;
                b.residual_note = Some("recorded but not run".into());
            }
        }
        let r = AgentExecGate::admit(Some(&att), &prod_id());
        assert_eq!(
            r.unwrap_err(),
            GateRefusal::ProductionBackendNotExercised {
                backend: Backend::FirecrackerMicrovm,
            }
        );
    }

    #[test]
    fn a_different_rootfs_digest_must_be_redrilled() {
        let att = green_attestation();
        let mut id = prod_id();
        id.rootfs_sha256 =
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef0".into();
        let r = AgentExecGate::admit(Some(&att), &id);
        assert!(matches!(
            r.unwrap_err(),
            GateRefusal::IdentityMismatch { .. }
        ));
    }

    #[test]
    fn a_different_kernel_digest_must_be_redrilled() {
        let att = green_attestation();
        let mut id = prod_id();
        id.kernel_sha256 =
            "cafef00dcafef00dcafef00dcafef00dcafef00dcafef00dcafef00dcafef00d0".into();
        let r = AgentExecGate::admit(Some(&att), &id);
        assert!(matches!(
            r.unwrap_err(),
            GateRefusal::IdentityMismatch { .. }
        ));
    }

    #[test]
    fn a_different_corpus_version_must_be_redrilled() {
        let att = green_attestation();
        let mut id = prod_id();
        id.corpus_version = CORPUS_VERSION + 1;
        let r = AgentExecGate::admit(Some(&att), &id);
        assert!(matches!(
            r.unwrap_err(),
            GateRefusal::IdentityMismatch { .. }
        ));
    }

    #[test]
    fn the_refusal_is_loud_and_self_describing() {
        assert!(GateRefusal::NoAttestation
            .to_string()
            .contains("no green attestation ⇒ no untrusted compute"));
        assert!(GateRefusal::Escapes { total_escapes: 2 }
            .to_string()
            .contains("REFUSING ALL"));
    }
}
