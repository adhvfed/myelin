//! # `escape_gate` — the AG-D4 / CI-T1 hard escape GATE on the Fabric's exec dispatch (AG-P17 → P-229, M2-C)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §2.2 guarantee 4 (the isolation floor + **the real-kernel escape drill — the single hard go/no-go
//! before ANY untrusted customer code runs, CI *or* agent**) + §9 row D-4. **Reconciliation:**
//! `00-reconciliation-decisions.md` X-6 (the escape drill gates BOTH kinds; `ToolHands::exec` **is**
//! CI's `kind=agent` job). **Contract:** `contract-index.md` row 8.4 (the real-kernel escape drill
//! gates both kinds — CONSUMED here as the gate), 1.6 (the `no-host-exec` lint, re-asserted by the
//! exec seam). **Drill:** `01-whole-system-e2e-and-drill-catalogue.md` row **AG-D4 / CI-T1**.
//!
//! ## What this module IS — the CONSUMPTION of CI's green attestation as a fail-closed gate
//! CI owns the runner + the real-kernel escape drill (ADR-20 / CI-P5 → P-239). The drill, run on a
//! REAL Firecracker microVM against the full adversarial corpus, emits an
//! [`EscapeAttestation`](myelin_ci_sandbox::EscapeAttestation) ONLY when it is genuinely green (ZERO
//! escapes, every catalogued attack contained). **This module is the FABRIC half of AG-P17:** it
//! CONSUMES that attestation — it does **not** re-implement the drill, nor fork the attestation type —
//! and turns it into a **fail-closed gate on the exec dispatch path**:
//!
//! > **No green AG-D4 attestation for the production backend ⇒ no untrusted compute.**
//!
//! The Fabric REFUSES to dispatch a `kind=agent` compute job unless a valid GREEN `EscapeAttestation`
//! exists for the production backend (Firecracker), with `total_escapes == 0` and a matching
//! kernel / rootfs / corpus identity. The check keys on the REAL attestation artifact / type — it is
//! **never** a hardcoded `true` (a [`AgentExecGate`] cannot be constructed without an attestation that
//! passes [`AgentExecGate::admit`], and there is no other constructor — the gate is fail-closed in the
//! TYPE, exactly like the routing split in [`crate::exec`]).
//!
//! ## Why the gate keys on identity, not merely "green"
//! ZERO escapes is BOTH the floor and the full answer (there is NO threshold below it), and the gate
//! is a **PERMANENT GATE re-run on every backend / image / kernel change** (untrusted-code execution
//! is a never-"done" surface, EI-04 §5). So a green attestation from a DIFFERENT image / kernel /
//! corpus does NOT admit the current production backend — the gate verifies the attestation describes
//! the *production* backend it is about to dispatch onto ([`ProductionBackendId`]): the gate backend
//! is Firecracker, the rootfs/kernel digests match the runner's pinned images, and the corpus version
//! matches the drilled corpus. A mismatch is a structural REFUSAL (the drill must be re-run for the
//! new identity), never a silent admit.
//!
//! ## Floors named (per AG-P17 — there is NO floor on AG-D4)
//! - **There is NO floor on AG-D4** — ZERO escapes is BOTH the floor and the full answer; it is a
//!   **PERMANENT GATE** re-run on every backend / image / kernel change forever.
//! - The **M4 re-confirm on the prod CI image is AG-P21 (→ P-348)** (CI side CI-P27 / P-348).
//! - The **real `LlmAgentRuntime` running its compute against this hardened runner is post-M5
//!   (AG-P25)** — this gate guards the runner the real runtime will eventually drive.
//! - **Continuous fuzzing + the full CVE corpus + a pre-GA third-party pentest** remain ongoing
//!   residuals on top of this gate (never "done"). The CI side proved the gate on a real microVM
//!   (CI-P5 → P-239); these residuals are carried IN WRITING in the attestation's `residuals` field.
//!
//! ## DB-free / VM-free
//! This module boots no guest and touches no DB: it consumes an already-emitted [`EscapeAttestation`]
//! value (the JSON the P-239 drill wrote, or one parsed from the artifact path). So `cargo build
//! --workspace` + the default `cargo test` stay green WITHOUT booting a VM; the `integration`-gated
//! test (`tests/integration_escape_gate.rs`) consumes the REAL attestation the P-239 drill produced.

use myelin_ci_sandbox::{Backend, EscapeAttestation};

/// The identity of the PRODUCTION backend the Fabric is about to dispatch a `kind=agent` job onto —
/// the facts a green AG-D4 attestation MUST match for the gate to admit. The drill is re-run on every
/// change to any of these (the permanent gate), so a green attestation for one identity does NOT
/// admit a different one.
///
/// These come from the production runner's pinned images (the Firecracker backend, CI-P2 → P-237):
/// the guest rootfs + kernel sha256 digests, the kernel version, and the adversarial corpus version
/// the drill exercised. The Fabric builds this from the SAME pinned configuration the runner launches
/// the job under, so "the attestation describes the backend we are about to use" is verifiable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionBackendId {
    /// The production backend kind — MUST be [`Backend::FirecrackerMicrovm`] (the GATE backend; gVisor
    /// is the named second backend CI-P28, drilled separately, not the production default here).
    pub backend: Backend,
    /// The sha256 of the guest rootfs image the production job runs in (re-run on every image change).
    pub rootfs_sha256: String,
    /// The sha256 of the guest kernel image (re-run on every kernel change).
    pub kernel_sha256: String,
    /// The adversarial corpus version the production gate requires (re-run on every corpus change).
    pub corpus_version: u32,
}

/// Why the AG-D4 gate REFUSED to admit untrusted compute. Every refusal is LOUD + self-describing
/// (never a swallowed pass): a missing / non-green / identity-mismatched attestation is a structural
/// no-go that blocks ALL untrusted compute (a red AG-D4 blocks ALL of M3+, EI-04 §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateRefusal {
    /// No attestation at all — the drill has not been run for the production backend. Fail-closed:
    /// no green attestation ⇒ no untrusted compute (the structural default is REFUSE).
    NoAttestation,
    /// The attestation is not the AG-D4 / CI-T1 green-escape artifact (wrong artifact kind / drill id)
    /// — a foreign or malformed artifact never admits.
    WrongArtifact {
        /// The artifact kind tag found (vs the expected `ag-d4-green-escape-attestation`).
        found_artifact: String,
        /// The drill id found (vs the expected `AG-D4 / CI-T1`).
        found_drill: String,
    },
    /// The attestation reports a NON-ZERO escape count — a red AG-D4 (catastrophic). ZERO escapes is
    /// the gate predicate; any escape is a dated no-go that blocks all untrusted compute.
    Escapes {
        /// The non-zero escape count read from the attestation.
        total_escapes: u32,
    },
    /// The attestation's gate backend is not the production backend (e.g. it proved gVisor, not the
    /// Firecracker production default) — the GATE is only admitted on the production backend.
    GateBackendMismatch {
        /// The gate backend the attestation was proven on.
        attested: Backend,
        /// The production backend the Fabric is about to dispatch onto.
        expected: Backend,
    },
    /// The production backend was NOT actually exercised in the drill run (recorded as a residual, not
    /// run on real silicon) — a deferred backend never admits production compute.
    ProductionBackendNotExercised {
        /// The production backend that must have been genuinely exercised.
        backend: Backend,
    },
    /// The attestation's image / kernel / corpus identity does not match the production backend the
    /// Fabric is about to dispatch onto — the drill must be RE-RUN for the new identity (the permanent
    /// gate). A green attestation for a different identity does NOT admit this one.
    IdentityMismatch {
        /// A human-readable description of which identity field mismatched.
        detail: String,
    },
}

impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateRefusal::NoAttestation => write!(
                f,
                "AG-D4 / CI-T1 GATE: no green escape attestation for the production backend — \
                 REFUSING all untrusted compute (no green attestation ⇒ no untrusted compute; \
                 the gate is fail-closed, agent-fabric.md §2.2 guarantee 4)"
            ),
            GateRefusal::WrongArtifact { found_artifact, found_drill } => write!(
                f,
                "AG-D4 / CI-T1 GATE: attestation is not the green-escape artifact \
                 (artifact=`{found_artifact}` drill=`{found_drill}`) — REFUSING untrusted compute"
            ),
            GateRefusal::Escapes { total_escapes } => write!(
                f,
                "AG-D4 / CI-T1 GATE: a RED attestation with {total_escapes} escape(s) — REFUSING ALL \
                 untrusted compute (one escape is catastrophic; a red AG-D4 blocks all of M3+, \
                 EI-04 §5). ZERO escapes is both the floor and the full answer."
            ),
            GateRefusal::GateBackendMismatch { attested, expected } => write!(
                f,
                "AG-D4 / CI-T1 GATE: the attestation proves backend `{}`, not the production backend \
                 `{}` — REFUSING untrusted compute (the gate is admitted only on the production \
                 backend)",
                attested.key(),
                expected.key()
            ),
            GateRefusal::ProductionBackendNotExercised { backend } => write!(
                f,
                "AG-D4 / CI-T1 GATE: the production backend `{}` was NOT exercised on real silicon in \
                 this drill (recorded as a residual) — REFUSING untrusted compute (a deferred backend \
                 never admits production compute)",
                backend.key()
            ),
            GateRefusal::IdentityMismatch { detail } => write!(
                f,
                "AG-D4 / CI-T1 GATE: the attestation's image/kernel/corpus identity does not match \
                 the production backend ({detail}) — REFUSING untrusted compute (the drill must be \
                 RE-RUN for the new identity; the gate is permanent, re-run on every \
                 backend/image/kernel change)"
            ),
        }
    }
}

impl std::error::Error for GateRefusal {}

/// The AG-D4 / CI-T1 hard escape GATE, admitted for ONE production backend identity. Its very
/// existence is the proof that a valid GREEN attestation matching the production backend was
/// consumed — it can ONLY be built by [`AgentExecGate::admit`], which REFUSES (returns
/// [`GateRefusal`]) for a missing / non-green / mismatched attestation. **There is no other
/// constructor**, so a Fabric exec path that holds an [`AgentExecGate`] has, by construction, a green
/// AG-D4 attestation for exactly the backend it dispatches onto — the fail-closed property is encoded
/// in the TYPE (no hardcoded `true`, no green claimed over a red).
///
/// This mirrors [`SandboxJob`](crate::exec::SandboxJob) (only `compute` can build one): the two
/// type-level gates meet on the exec dispatch path — a job reaches the kernel sandbox ONLY if it is
/// `compute` (the routing split) AND the production backend has a green AG-D4 attestation (this gate).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentExecGate {
    /// The production backend identity this gate admitted (the attestation matched it green).
    backend_id: ProductionBackendId,
    /// The date the admitting attestation was minted (the dated green proof — observability is part
    /// of the pass, EI-01 §3).
    attested_date: String,
    /// The kernel version the admitting attestation was proven on (carried for the green log line).
    kernel_version: String,
}

impl AgentExecGate {
    /// **The gate predicate — the ONLY way to obtain an [`AgentExecGate`].** Admit untrusted compute
    /// on `backend_id` IFF the supplied attestation is a genuinely GREEN AG-D4 / CI-T1 escape
    /// attestation for exactly that production backend:
    ///
    /// 1. an attestation is PRESENT (`Some`) — else [`GateRefusal::NoAttestation`] (fail-closed: the
    ///    structural default with no attestation is REFUSE);
    /// 2. it is the green-escape ARTIFACT (`artifact == "ag-d4-green-escape-attestation"` and
    ///    `drill == "AG-D4 / CI-T1"`) — else [`GateRefusal::WrongArtifact`];
    /// 3. `total_escapes == 0` — else [`GateRefusal::Escapes`] (a red AG-D4 NEVER admits);
    /// 4. the attestation's `gate_backend` is the production backend — else
    ///    [`GateRefusal::GateBackendMismatch`];
    /// 5. that production backend was ACTUALLY exercised on real silicon (`exercised == true`) — else
    ///    [`GateRefusal::ProductionBackendNotExercised`] (a residual backend never admits);
    /// 6. the rootfs / kernel digests + corpus version MATCH `backend_id` — else
    ///    [`GateRefusal::IdentityMismatch`] (the drill must be re-run for a new identity; the gate is
    ///    permanent).
    ///
    /// `attestation` is `Option<&EscapeAttestation>` so the absent case (no drill yet) is the
    /// fail-closed REFUSE by construction — the consumer cannot accidentally admit by forgetting to
    /// load the artifact. The check keys on the REAL [`EscapeAttestation`] fields — never a hardcoded
    /// `true`.
    pub fn admit(
        attestation: Option<&EscapeAttestation>,
        backend_id: &ProductionBackendId,
    ) -> Result<AgentExecGate, GateRefusal> {
        // (1) PRESENT — the fail-closed default with no attestation is REFUSE.
        let att = attestation.ok_or(GateRefusal::NoAttestation)?;

        // (2) the green-escape ARTIFACT — a foreign / malformed artifact never admits.
        if att.artifact != "ag-d4-green-escape-attestation" || att.drill != "AG-D4 / CI-T1" {
            return Err(GateRefusal::WrongArtifact {
                found_artifact: att.artifact.clone(),
                found_drill: att.drill.clone(),
            });
        }

        // (3) ZERO escapes — a red AG-D4 NEVER admits (one escape is catastrophic).
        if att.total_escapes != 0 {
            return Err(GateRefusal::Escapes {
                total_escapes: att.total_escapes,
            });
        }

        // (4) the GATE backend is the production backend.
        if att.gate_backend != backend_id.backend {
            return Err(GateRefusal::GateBackendMismatch {
                attested: att.gate_backend,
                expected: backend_id.backend,
            });
        }

        // (5) the production backend was ACTUALLY exercised on real silicon (not a residual).
        let exercised = att
            .backends
            .iter()
            .any(|b| b.backend == backend_id.backend && b.exercised);
        if !exercised {
            return Err(GateRefusal::ProductionBackendNotExercised {
                backend: backend_id.backend,
            });
        }

        // (6) the image / kernel / corpus IDENTITY matches the production backend the Fabric is about
        //     to dispatch onto (the permanent gate: a different identity must be re-drilled).
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

    /// Parse + admit from the raw attestation JSON artifact (the form the P-239 drill writes to
    /// `target/ag-d4-attestation/<date>.json`). A malformed / unparseable artifact is treated as
    /// [`GateRefusal::NoAttestation`] (fail-closed: an artifact we cannot read is no proof). Keys on
    /// the REAL [`EscapeAttestation`] deserialization — never a hardcoded admit.
    pub fn admit_from_json(
        json: &str,
        backend_id: &ProductionBackendId,
    ) -> Result<AgentExecGate, GateRefusal> {
        match serde_json::from_str::<EscapeAttestation>(json) {
            Ok(att) => AgentExecGate::admit(Some(&att), backend_id),
            // An unparseable artifact is NOT a green proof — fail-closed to NoAttestation.
            Err(_) => Err(GateRefusal::NoAttestation),
        }
    }

    /// The production backend identity this gate admitted (read-only).
    pub fn backend_id(&self) -> &ProductionBackendId {
        &self.backend_id
    }

    /// The one-line `[AG-D4 GATE OPEN] …` proof line (observability is part of the pass, EI-01 §3):
    /// the dated, identity-stamped record that untrusted compute is admitted because the production
    /// backend has a green AG-D4 attestation.
    pub fn open_line(&self) -> String {
        format!(
            "[AG-D4 GATE OPEN] date={} gate-backend={} kernel={} corpus-version={} \
             rootfs-sha256={:.16}… — untrusted compute admitted (ZERO escapes attested)",
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

    /// The production backend identity the gate is admitted for (matches the P-239 drill artifact).
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

    /// A real green drill report (every catalogued attack CONTAINED, corpus completed) — minted from
    /// the corpus parser, NEVER hardcoded.
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

    // ───────────────────────────── fail-closed: REFUSE without a green attestation ───────────────

    #[test]
    fn no_attestation_refuses_all_untrusted_compute() {
        // THE headline fail-closed property: with NO attestation, the gate REFUSES (no green
        // attestation ⇒ no untrusted compute). The structural default is REFUSE.
        let r = AgentExecGate::admit(None, &prod_id());
        assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
    }

    #[test]
    fn a_red_attestation_can_never_be_minted_and_so_can_never_admit() {
        // A red drill cannot even produce an EscapeAttestation (the ci-sandbox guard refuses to mint
        // over a red drill) — so a red AG-D4 has NO artifact to feed the gate, and the gate stays
        // closed (NoAttestation). This is the structural fail-closed at the source.
        let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
        for atk in CORPUS {
            console.push_str(&format!("{} CONTAINED\n", atk.id));
        }
        // flip ONE attack to ESCAPED → a red drill.
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
        // With no minted artifact, the gate is fail-closed.
        let r = AgentExecGate::admit(None, &prod_id());
        assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
    }

    #[test]
    fn a_hand_forged_red_escape_count_is_refused() {
        // Defence-in-depth: even if a forged artifact carried total_escapes > 0 (bypassing the
        // mint-time guard), the gate REFUSES on the escape count directly. The gate never trusts
        // "green" without checking the escape count itself.
        let mut att = green_attestation();
        att.total_escapes = 1;
        let r = AgentExecGate::admit(Some(&att), &prod_id());
        assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 1 });
    }

    // ───────────────────────────── a GREEN attestation admits ────────────────────────────────────

    #[test]
    fn a_green_matching_attestation_admits_untrusted_compute() {
        let att = green_attestation();
        let gate = AgentExecGate::admit(Some(&att), &prod_id())
            .expect("a green, identity-matched attestation admits");
        assert_eq!(gate.backend_id().backend, Backend::FirecrackerMicrovm);
        // Observability: the dated green proof line.
        assert!(gate.open_line().starts_with("[AG-D4 GATE OPEN]"));
        assert!(gate.open_line().contains("ZERO escapes"));
    }

    #[test]
    fn admit_from_the_real_json_artifact_form() {
        // The gate consumes the SAME JSON the P-239 drill writes (round-trip through the artifact).
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

    // ───────────────────────────── identity is load-bearing (permanent gate) ─────────────────────

    #[test]
    fn a_wrong_artifact_kind_never_admits() {
        let mut att = green_attestation();
        att.artifact = "some-other-artifact".into();
        let r = AgentExecGate::admit(Some(&att), &prod_id());
        assert!(matches!(r.unwrap_err(), GateRefusal::WrongArtifact { .. }));
    }

    #[test]
    fn a_gvisor_only_attestation_does_not_admit_the_firecracker_production_backend() {
        // A green attestation whose gate backend is gVisor does NOT admit the Firecracker production
        // default — the gate is admitted only on the production backend.
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
        // If the production backend is in the attestation but was NOT exercised on real silicon
        // (recorded as a residual), it never admits production compute.
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
        // The permanent gate: a green attestation for a DIFFERENT image does not admit a changed
        // production image — it must be re-drilled.
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
        // Every refusal renders a LOUD message (never a swallowed pass).
        assert!(GateRefusal::NoAttestation
            .to_string()
            .contains("no green attestation ⇒ no untrusted compute"));
        assert!(GateRefusal::Escapes { total_escapes: 2 }
            .to_string()
            .contains("REFUSING ALL"));
    }
}
