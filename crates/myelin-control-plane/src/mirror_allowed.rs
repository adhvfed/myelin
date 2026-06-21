//! # `mirror_allowed` — the outbound push-mirror residency gate (C-4, deny-by-default)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` **§7.4 in full**
//! (the outbound push-mirror residency gate — `mirror_allowed(tenant_id, mirror_target) → Allow |
//! Deny{reason}`; an extra-EU PII-bearing target is denied by default; consults GDPR's
//! `transfer_allowed`/`subprocessors` registry; **the split** — the control plane decides *"does this
//! target cross the residency boundary?"* (it knows the tenant's region + the target's region), GDPR
//! owns *"is this transfer lawful?"*; a crossing push without a `transfer_allowed` entry is REFUSED,
//! not logged-and-allowed). Contract-index row 10.5 (the `transfer_allowed`/`subprocessors` registry +
//! the outbound-mirror gate co-owned half, **C-4** — Tenancy owns the mirror half). Reconciliation §10
//! (the C-4 mirror gate rationale). Failure-modes §9 (the C-4 drill rides the **D-CP-3 / CP-D3**
//! residency family: 0 unauthorised cross-residency mirror pushes).
//!
//! ## The doctrine this implements
//! - VISION §1 — EU-sovereign: a PII-bearing byte does not leave the region absent a **registered**
//!   lawful basis. An outbound push-mirror (a Git mirror config pushing a repo — commit author
//!   identity, message bodies — to a foreign host) is a residency-boundary crossing for PII-bearing
//!   content, so it is **policy-gated at the control plane**.
//! - `external-insights/01` §2 — cross-residency PII egress via a mirror is **stop-the-bleeding**:
//!   deny-by-default (the gate's resting state is `Deny`; an `Allow` is the exception that needs a
//!   recorded lawful basis), and §5 — a refused mirror is **loud** ([`MirrorDecision::Deny`] carries
//!   the reason), never logged-and-allowed.
//!
//! ## What this prompt (P-CP-16 / P-251) ships
//! 1. **[`mirror_allowed`]** — `mirror_allowed(tenant_id, mirror_target, &policy) → MirrorDecision`.
//!    The control plane resolves the tenant's residency region (the `tenant_placement` region of
//!    record — the SAME immutable region the four-layer enforcement pins), reads the mirror target's
//!    region, and makes the **residency-boundary decision**: a target in the tenant's own region is no
//!    crossing (allowed without a policy consult); a target in a *different* region **crosses the
//!    boundary** and is gated. For a crossing it consults the injected GDPR [`TransferPolicy`] — the
//!    `transfer_allowed` half — and permits the transfer **ONLY** if the policy records a lawful basis
//!    (a within-EU acceleration target, or an extra-EU target with a recorded transfer mechanism). A
//!    crossing push without a `transfer_allowed` entry is **denied** (a `Deny{reason}`), not
//!    logged-and-allowed.
//! 2. **The ownership split** is structural, not by convention: the control plane NEVER answers "is
//!    this transfer lawful?" — it answers "does this target cross the residency boundary?" (it has the
//!    tenant's region + the target's region) and DELEGATES the lawful-basis question to the GDPR
//!    [`TransferPolicy`] port. The port is a TRAIT this crate owns; the production implementor is
//!    GDPR's `TransferGate` (`myelin-gdpr-service`) — wired at the seam (the CDC pair proves it), so
//!    the control-plane production DAG takes no runtime dependency on the GDPR service crate.
//! 3. **The unauthorised-push counter** ([`MirrorGate::unauthorised_push_count`]) — the C-4 drill's
//!    most-load-bearing zero: every crossing push the gate **denies** that a caller would have made is
//!    a prevented unauthorised cross-residency push; the gate counts the denials it issued so the drill
//!    asserts `unauthorised_pushes == 0` (0 unauthorised cross-residency mirror pushes slip through).
//!
//! ## Floor named (VISION §3, recorded in writing)
//! The default-deny gate ships **regardless**. The counsel-ratified `transfer_allowed` entries that
//! would permit a *specific* extra-EU mirror are **`[OPEN — LEGAL]`** (Schrems II / GDPR Art. 44–49) —
//! one legally-reviewed, ratified statement per target, a **parallel (legal)** track, **NOT** an
//! engineering gate. The engineering contract here is: absent such a recorded entry, the gate denies.
//! Recording an entry is [`TransferPolicy::transfer_allowed`] returning a lawful basis for that target;
//! the *legal sufficiency* of that basis is the open legal follow-on, owned by GDPR/Audit + counsel.
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The `mirror_allowed` **deny-by-default** decision is mandatory-core: a mutation that flips the
//! deny-by-default to allow (or that stops consulting `transfer_allowed`, or that drops the
//! fail-closed unknown-tenant deny) makes a cross-residency PII egress via a mirror possible — the
//! stop-the-bleeding zero (EI-01 §2). The floor is **>= 80%**. Achieved (measured):
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/mirror_allowed.rs` →
//! **7 caught, 1 unviable, 0 missed = 100% of the 7 viable mutants**. Every mutation of the gate's
//! decision (the same-region branch, the `transfer_allowed` consult, the deny-by-default, the
//! unknown-tenant fail-closed, the prevented-push counter) is killed by an assertion in the unit
//! tests + the C-4 drill + the CDC pair.
//!
//! ## Coherence note (EI-01 §7 — no parallel implementation)
//! This crate does **not** re-derive the EU/EEA sovereignty boundary or a second `transfer_allowed`
//! policy. There is ONE such policy in the platform — GDPR's `TransferGate` (`is_eea_region` +
//! recorded transfer mechanisms, `myelin-gdpr-service`). `mirror_allowed` CONSUMES it through the
//! [`TransferPolicy`] port (the consumer half of contract 10.5). The control plane's distinct,
//! non-duplicated job is the **residency-boundary decision** (same-region ⇒ no crossing; cross-region
//! ⇒ gated) — which is a function of the tenant's region (a control-plane stored fact) and the target's
//! region, neither of which GDPR owns.

use myelin_tenancy::{Region, TenantId};

use crate::registry::Registry;

/// **A resolved outbound push-mirror target (§7.4).** A Git mirror config pushes a repo to a foreign
/// host; this is the control plane's PII-free view of that target: an opaque host identifier + the
/// region the host resolves to. The control plane KNOWS the target's region (it resolves the mirror
/// host to its region of record — the residency input to the boundary decision); it does NOT carry the
/// repo content (the gate is a routing/residency decision, never a payload path).
///
/// PII-free by construction: a host string (a DNS name / endpoint identifier, not a person) + a region
/// code. No principal, no commit body — the gate decides residency, it never touches PII.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorTarget {
    /// The opaque mirror host identifier (a DNS name / endpoint — PII-free).
    pub host: String,
    /// The region the mirror host resolves to (the residency input — the control plane resolves the
    /// host to its region of record; this is what makes "does this cross the boundary?" answerable).
    pub region: Region,
}

impl MirrorTarget {
    /// A mirror target from a host identifier + the region it resolves to.
    pub fn new(host: impl Into<String>, region: Region) -> MirrorTarget {
        MirrorTarget {
            host: host.into(),
            region,
        }
    }
}

/// **The verdict of the outbound push-mirror residency gate (§7.4).** Either the mirror is **allowed**
/// (the target is in the tenant's own region — no crossing — OR a crossing WITH a recorded lawful basis
/// in the `transfer_allowed` registry), or it is **denied** with a **loud** reason (the §5/§2 doctrine:
/// a refused mirror is loud, never logged-and-allowed). `Deny` is the gate's resting state for a
/// residency-boundary crossing without a recorded lawful basis (deny-by-default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorDecision {
    /// The mirror is permitted: either the target is in the tenant's own residency region (no boundary
    /// crossed), or the target crosses the boundary but the `transfer_allowed` registry records a
    /// lawful basis for it (a within-EU acceleration target, or an extra-EU target with a recorded
    /// transfer mechanism). Carries the reason the allow was granted (observability — an allow is as
    /// auditable as a deny).
    Allow {
        /// Why the mirror was allowed (PII-free, for the audit trail).
        reason: MirrorAllowReason,
    },
    /// The mirror is **refused** — a residency-boundary crossing for PII-bearing content WITHOUT a
    /// recorded lawful basis (the §7.4 / §5.2 deny-by-default). The `reason` is loud (§5: never
    /// logged-and-allowed). A caller (the Git mirror feature) that receives this MUST NOT push.
    Deny {
        /// Why the mirror was denied (PII-free — region codes + the policy verdict, never a payload).
        reason: MirrorDenyReason,
    },
}

impl MirrorDecision {
    /// Whether the mirror is allowed. A `false` here is a HARD refusal the caller must honour — it must
    /// not push PII-bearing content when the gate denies (the gate is the residency boundary).
    pub fn is_allowed(&self) -> bool {
        matches!(self, MirrorDecision::Allow { .. })
    }
}

/// **Why an outbound mirror was ALLOWED (§7.4, PII-free).** Either the target did not cross the
/// residency boundary at all, or the crossing had a recorded lawful basis. Both are auditable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorAllowReason {
    /// The target is in the tenant's OWN residency region — no residency boundary is crossed, so no
    /// `transfer_allowed` consult is needed (the data never leaves the region). The control plane's
    /// half of the decision answered "no crossing".
    SameRegion {
        /// The (shared) region of the tenant and the target.
        region: Region,
    },
    /// The target crosses the residency boundary, BUT the GDPR `transfer_allowed` registry records a
    /// lawful basis for it (a within-EU acceleration target, or an extra-EU target with a recorded
    /// transfer mechanism). The control plane decided "crossing"; GDPR's policy decided "lawful".
    LawfulTransfer {
        /// The tenant's residency region (the boundary's near side).
        tenant_region: Region,
        /// The target's region (the boundary's far side — the crossing is permitted by policy).
        target_region: Region,
    },
}

/// **Why an outbound mirror was DENIED (§7.4, PII-free, loud).** A refusal carries the offending region
/// codes + the policy verdict so the deny is observable (§5: loud, never silently swallowed) — never a
/// payload, so a denial reason is `control-plane-pii-free` by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorDenyReason {
    /// The target crosses the tenant's residency boundary AND the GDPR `transfer_allowed` registry
    /// records NO lawful basis for it — the §7.4 / §5.2 deny-by-default. This is the headline refusal:
    /// a crossing push without a `transfer_allowed` entry is refused, not logged-and-allowed.
    NoLawfulTransfer {
        /// The tenant's residency region (the near side of the crossing).
        tenant_region: Region,
        /// The target's region (the far side — the byte would land here; denied absent a lawful basis).
        target_region: Region,
    },
    /// The control plane could not resolve the tenant's residency region (no `tenant_placement` row) —
    /// the gate **fails closed** (a tenant with no placement of record cannot mirror anywhere: there is
    /// no region to anchor the boundary decision, so deny). 0 mirrors slip through on an unknown tenant.
    UnknownTenant {
        /// The tenant whose placement of record is absent.
        tenant_id: TenantId,
    },
}

/// **The GDPR `transfer_allowed` policy port (the consumer half of contract 10.5).** The control plane
/// owns the *residency-boundary* decision (`mirror_allowed`); it DELEGATES the *lawful-transfer*
/// decision to this port — the production implementor is GDPR's `TransferGate`
/// (`myelin-gdpr-service`). Keeping it a trait this crate owns means the control-plane PRODUCTION DAG
/// takes **no runtime dependency** on the GDPR service crate (the CDC pair / the drill wire the real
/// `TransferGate` as the implementor — `substrate_is_root()` preserved).
///
/// The split (§7.4): the control plane asks this port "for a transfer of PII-bearing content TO
/// `target`, is there a recorded lawful basis?" — and HONOURS the answer. The control plane never
/// re-derives the EU/EEA boundary or the recorded-mechanism set (that is GDPR's, EI-01 §7).
pub trait TransferPolicy {
    /// `transfer_allowed(target) → bool` — does the GDPR registry record a lawful basis for a PII
    /// transfer to `target`? `true` ⇒ a within-EU acceleration target OR an extra-EU target with a
    /// recorded transfer mechanism (a recorded TIA + SCCs / adequacy, `[OPEN — LEGAL]` for the legal
    /// sufficiency); `false` ⇒ extra-EU by default with no recorded mechanism (the deny-by-default).
    ///
    /// The control plane calls this ONLY for a residency-boundary crossing (a same-region mirror never
    /// consults the policy — the data never leaves the region).
    fn transfer_allowed(&self, target: &Region) -> bool;
}

/// **The outbound push-mirror residency gate (§7.4, C-4).** Holds the registry (the tenant's region of
/// record — the residency input) + the count of unauthorised cross-residency pushes the gate denied
/// (the C-4 drill's most-load-bearing zero). The gate is **deny-by-default**: its resting verdict for a
/// boundary crossing without a recorded lawful basis is `Deny`.
///
/// The gate borrows the [`Registry`] (the source of the tenant's region of record) and a
/// [`TransferPolicy`] (GDPR's `transfer_allowed`) at decision time — it does not own them, so it does
/// not duplicate either the placement registry or the transfer policy (EI-01 §7).
#[derive(Default)]
pub struct MirrorGate {
    /// The running count of unauthorised cross-residency mirror pushes the gate **prevented** — every
    /// `Deny` of a boundary crossing without a recorded lawful basis (the C-4 telemetry: the drill
    /// asserts the value a regression that *allowed* such pushes would inflate stays at its zero on the
    /// permitted set, and counts the denials on the adversarial set). PII-free scalar.
    unauthorised_pushes_prevented: u64,
}

impl MirrorGate {
    /// A fresh mirror gate — deny-by-default (no pushes prevented yet).
    pub fn new() -> MirrorGate {
        MirrorGate::default()
    }

    /// **`mirror_allowed(tenant_id, mirror_target, &policy) → MirrorDecision` (§7.4, C-4).**
    ///
    /// The decision, in the §7.4 ownership split:
    ///
    /// 1. **Resolve the tenant's residency region** from the `tenant_placement` row of record (the
    ///    SAME immutable region the four-layer enforcement pins). No placement ⇒ **deny, fail-closed**
    ///    ([`MirrorDenyReason::UnknownTenant`]) — a tenant with no region of record cannot anchor a
    ///    boundary decision, so 0 mirrors slip through.
    /// 2. **The residency-boundary decision (the control plane's half):** if the target's region equals
    ///    the tenant's region, **no boundary is crossed** — the byte never leaves the region — so the
    ///    mirror is **allowed** without consulting the transfer policy
    ///    ([`MirrorAllowReason::SameRegion`]).
    /// 3. **A crossing (the gated path):** the target is in a *different* region — this crosses the
    ///    residency boundary for PII-bearing content. Consult the GDPR [`TransferPolicy`]
    ///    (`transfer_allowed`, the lawful-transfer half): if it records a lawful basis for the target,
    ///    **allow** ([`MirrorAllowReason::LawfulTransfer`]); else **deny by default**
    ///    ([`MirrorDenyReason::NoLawfulTransfer`]) and count the prevented unauthorised push (the C-4
    ///    zero). A crossing push without a `transfer_allowed` entry is **refused**, not
    ///    logged-and-allowed.
    ///
    /// `&mut self` because a default-deny of a crossing bumps the prevented-push counter (the drill's
    /// observable zero — observability is part of the pass, EI-01 §3).
    pub fn mirror_allowed(
        &mut self,
        registry: &Registry,
        tenant_id: &TenantId,
        mirror_target: &MirrorTarget,
        policy: &dyn TransferPolicy,
    ) -> MirrorDecision {
        // (1) Resolve the tenant's region of record — the residency anchor. Fail closed if absent.
        let Some(placement) = registry.placement(tenant_id) else {
            return MirrorDecision::Deny {
                reason: MirrorDenyReason::UnknownTenant {
                    tenant_id: tenant_id.clone(),
                },
            };
        };
        let tenant_region = placement.region.clone();

        // (2) The residency-boundary decision (the control plane's half): same region ⇒ no crossing.
        //     The data never leaves the region, so no transfer-policy consult is needed.
        if mirror_target.region == tenant_region {
            return MirrorDecision::Allow {
                reason: MirrorAllowReason::SameRegion {
                    region: tenant_region,
                },
            };
        }

        // (3) A residency-boundary CROSSING — consult GDPR's transfer_allowed (the lawful-transfer
        //     half). Permit ONLY if the registry records a lawful basis; else deny by default (loud).
        if policy.transfer_allowed(&mirror_target.region) {
            MirrorDecision::Allow {
                reason: MirrorAllowReason::LawfulTransfer {
                    tenant_region,
                    target_region: mirror_target.region.clone(),
                },
            }
        } else {
            // Deny-by-default: a crossing push without a transfer_allowed entry is REFUSED. Count the
            // unauthorised cross-residency push the gate just PREVENTED (the C-4 most-load-bearing zero).
            self.unauthorised_pushes_prevented += 1;
            MirrorDecision::Deny {
                reason: MirrorDenyReason::NoLawfulTransfer {
                    tenant_region,
                    target_region: mirror_target.region.clone(),
                },
            }
        }
    }

    /// **The count of unauthorised cross-residency mirror pushes the gate PREVENTED** — every
    /// boundary-crossing push denied by default (no recorded lawful basis). The C-4 drill asserts the
    /// number of unauthorised pushes that *slipped through* is **0**: this counter is the number the
    /// gate caught, and the drill proves a denied push never reaches the wire (the caller honours the
    /// `Deny`), so 0 unauthorised cross-residency pushes are made. PII-free scalar.
    pub fn unauthorised_pushes_prevented(&self) -> u64 {
        self.unauthorised_pushes_prevented
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{IsolationKind, PlacementStatus, TenantPlacement};
    use myelin_tenancy::CellId;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    /// A test [`TransferPolicy`] standing in for GDPR's `TransferGate`: within-EU/EEA regions are
    /// allowed structurally; extra-EU regions are denied UNLESS a recorded transfer mechanism is added.
    /// (The CDC pair `cdc_10_5_mirror_gate` wires the REAL `myelin_gdpr_service::TransferGate` — this
    /// in-test double mirrors its frozen behaviour so the unit tests do not pull the service crate.)
    struct PolicyDouble {
        /// The EU/EEA regions (allowed structurally) + extra-EU regions WITH a recorded mechanism.
        allowed: RefCell<BTreeSet<String>>,
    }

    impl PolicyDouble {
        fn new() -> PolicyDouble {
            // The EU/EEA structural set (a subset, mirroring `is_eea_region` for the regions under test).
            let mut allowed = BTreeSet::new();
            for r in ["fr-par", "nl-ams", "de-fra", "no-osl"] {
                allowed.insert(r.to_string());
            }
            PolicyDouble {
                allowed: RefCell::new(allowed),
            }
        }
        /// Record a transfer mechanism for an extra-EU target (the `[OPEN — LEGAL]` counsel-ratified
        /// entry — here the engineering fact that one exists).
        fn record_mechanism(&self, region: &str) {
            self.allowed.borrow_mut().insert(region.to_string());
        }
    }

    impl TransferPolicy for PolicyDouble {
        fn transfer_allowed(&self, target: &Region) -> bool {
            self.allowed.borrow().contains(target.as_str())
        }
    }

    fn registry_with(tenant: &str, region: &str, home: &str) -> Registry {
        use crate::schema::{Capacity, Cell, CellStatus};
        let mut reg = Registry::new();
        reg.insert_cell(Cell {
            cell_id: CellId::from_token(home),
            region: Region::new(region),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 10,
            version: 1,
            endpoint: format!("cell.{region}.{home}.myelin.eu"),
        });
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token(tenant),
            region: Region::new(region),
            home_cell: CellId::from_token(home),
            isolation_tier: IsolationKind::Pool,
            slug: tenant.into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token(home)],
        })
        .expect("a single-region placement is admitted");
        reg
    }

    /// **The headline: an extra-EU PII-bearing target is DENIED BY DEFAULT (§7.4 / §5.2).** ACME is
    /// pinned to eu-west (fr-par); a mirror target on an extra-EU host (us-east) has NO recorded
    /// `transfer_allowed` entry → the gate **denies** with a loud reason; the prevented-push counter
    /// increments (the byte never reaches the foreign host).
    #[test]
    fn extra_eu_target_denied_by_default() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();

        let target = MirrorTarget::new("github.com", Region::new("us-east"));
        let decision = gate.mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy);

        assert_eq!(
            decision,
            MirrorDecision::Deny {
                reason: MirrorDenyReason::NoLawfulTransfer {
                    tenant_region: Region::new("fr-par"),
                    target_region: Region::new("us-east"),
                },
            },
            "an extra-EU PII-bearing target with no transfer_allowed entry is denied by default (loud)"
        );
        assert!(!decision.is_allowed(), "the caller must NOT push on a Deny");
        assert_eq!(
            gate.unauthorised_pushes_prevented(),
            1,
            "the prevented unauthorised cross-residency push is counted (the C-4 zero)"
        );
    }

    /// **A crossing is PERMITTED ONLY when `transfer_allowed` records a lawful basis (§7.4).** The same
    /// extra-EU target (us-east) flips from denied to allowed once a transfer mechanism is recorded —
    /// proving the gate consults the registry and is not a blanket extra-EU block.
    #[test]
    fn extra_eu_target_allowed_only_with_recorded_lawful_basis() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();
        let target = MirrorTarget::new("github.com", Region::new("us-east"));

        // Before recording: denied.
        assert!(
            !gate
                .mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy)
                .is_allowed(),
            "denied by default before a lawful basis is recorded"
        );

        // The `[OPEN — LEGAL]` counsel-ratified entry is recorded (the engineering fact).
        policy.record_mechanism("us-east");
        let decision = gate.mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy);
        assert_eq!(
            decision,
            MirrorDecision::Allow {
                reason: MirrorAllowReason::LawfulTransfer {
                    tenant_region: Region::new("fr-par"),
                    target_region: Region::new("us-east"),
                },
            },
            "a crossing WITH a recorded transfer mechanism is permitted"
        );
        // The prevented-push counter stays at the one deny before recording (the allow did not bump it).
        assert_eq!(gate.unauthorised_pushes_prevented(), 1);
    }

    /// **A same-region mirror is allowed WITHOUT a policy consult (§7.4 — no crossing).** ACME (fr-par)
    /// mirrors to another fr-par host: the byte never leaves the region, so the gate allows it as
    /// `SameRegion` and never asks the transfer policy.
    #[test]
    fn same_region_target_allowed_without_policy_consult() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let mut gate = MirrorGate::new();

        /// A policy that PANICS if consulted — proving the same-region path never asks it.
        struct NeverConsulted;
        impl TransferPolicy for NeverConsulted {
            fn transfer_allowed(&self, _target: &Region) -> bool {
                panic!("the same-region path must NOT consult the transfer policy (no crossing)");
            }
        }

        let target = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
        let decision = gate.mirror_allowed(
            &reg,
            &TenantId::from_token("01J0ACME"),
            &target,
            &NeverConsulted,
        );
        assert_eq!(
            decision,
            MirrorDecision::Allow {
                reason: MirrorAllowReason::SameRegion {
                    region: Region::new("fr-par"),
                },
            },
            "a same-region mirror crosses no boundary — allowed without a policy consult"
        );
        assert_eq!(
            gate.unauthorised_pushes_prevented(),
            0,
            "a same-region allow prevents nothing — the zero holds"
        );
    }

    /// **A within-EU (cross-region) acceleration target is allowed via the policy (§7.4 / §5.3).** ACME
    /// (fr-par) mirrors to a within-EU host in a DIFFERENT region (nl-ams): this DOES cross the tenant's
    /// region boundary, so the policy is consulted — and `transfer_allowed` admits within-EU
    /// structurally, so it is allowed as a `LawfulTransfer` (within-EU acceleration is permitted).
    #[test]
    fn within_eu_cross_region_target_allowed_via_policy() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();

        let target = MirrorTarget::new("mirror.nl.example", Region::new("nl-ams"));
        let decision = gate.mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy);
        assert_eq!(
            decision,
            MirrorDecision::Allow {
                reason: MirrorAllowReason::LawfulTransfer {
                    tenant_region: Region::new("fr-par"),
                    target_region: Region::new("nl-ams"),
                },
            },
            "within-EU acceleration (a different EU region) is permitted by the policy (§5.3)"
        );
        assert_eq!(
            gate.unauthorised_pushes_prevented(),
            0,
            "a policy-permitted crossing prevents nothing"
        );
    }

    /// **An unknown tenant fails CLOSED (§5.3 — no region of record, deny).** A tenant with no
    /// `tenant_placement` row has no residency anchor, so the gate denies any mirror — 0 mirrors slip
    /// through on an unknown tenant.
    #[test]
    fn unknown_tenant_fails_closed() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();

        let target = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
        let decision = gate.mirror_allowed(&reg, &TenantId::from_token("01J0GHOST"), &target, &policy);
        assert_eq!(
            decision,
            MirrorDecision::Deny {
                reason: MirrorDenyReason::UnknownTenant {
                    tenant_id: TenantId::from_token("01J0GHOST"),
                },
            },
            "a tenant with no placement of record cannot mirror — fail closed"
        );
        assert!(!decision.is_allowed());
    }
}
