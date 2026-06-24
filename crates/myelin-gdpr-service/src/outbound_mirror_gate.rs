//! # The outbound push-mirror residency gate (GA-11) — P-GA-36 → global P-452 (M5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§5.3 in full** (*"the
//! outbound push-mirror residency gate — a mirror config targeting an extra-EU host for PII-bearing
//! content is **denied by default** at the same `transfer_allowed` gate that governs sub-processors
//! (contract 10.5). … **Within-EU CDN clone/bundle distribution is permitted** (Storage 11.2 NEW blob
//! class): clone bundles are content-addressed, region-pinned to the tenant's region; **no extra-EU
//! edge serves PII**. The gate distinguishes within-EU acceleration (allowed) from extra-EU
//! replication (denied by default)."*). Doctrine:
//! `external-insights/04-hard-problems.md` §1 (*the residual — independent off-platform clones a
//! third party holds — is named, not pretended-solved*) +
//! `external-insights/01-process-and-quality-doctrine.md` §2 (cross-residency PII egress via a mirror
//! is stop-the-bleeding: deny-by-default, the resting state is `Deny`).
//!
//! **Contract-index:** OWNS the **outbound-mirror gate POLICY leg of row 10.5** — this COMPLETES the
//! `transfer_allowed` policy floor P-GA-23 named (the policy ships in [`crate::registries`]; the Git
//! mirror SEAM it gates is M3/M4 and the gate is PROVEN end-to-end HERE at M5, GA-11). CONSUMES 11.2
//! (the within-EU CDN clone/bundle class — the [`OutboundConfigKind::CdnClone`] this gate admits) and
//! 12.x (the control-plane residency enforcement reading `transfer_allowed` — `myelin-control-plane`'s
//! `mirror_allowed`, the enforcement half; the CDC pair `tests/cdc_10_5_outbound_mirror_gate.rs`
//! proves the policy this gate ships REACHES that enforcement gate).
//!
//! ## What THIS prompt (P-GA-36, GA-11) ships — and what it REUSES (EI-01 §7, coherence)
//! The §5.2 `transfer_allowed` region-policy ([`crate::registries::TransferGate`] +
//! [`crate::registries::is_eea_region`]) ALREADY exists (P-GA-23); the control-plane enforcement gate
//! (`myelin_control_plane::MirrorGate::mirror_allowed`, P-251) and the Storage residency FLAG
//! (`myelin_storage::PushMirrorClass`, P-255) ALREADY exist. This module does **NOT** re-derive the
//! EU/EEA boundary, re-author a second `transfer_allowed` policy, or duplicate the control-plane
//! enforcement. What is genuinely NEW is the **§5.3 within-EU-acceleration-vs-extra-EU-replication
//! distinction made first-class on the GDPR policy side** — the piece §5.3 calls out that the bare
//! region gate does not by itself express:
//!
//! 1. **[`OutboundConfigKind`]** — the structural classification of an outbound replication config:
//!    a **`PushMirror`** (extra-EU REPLICATION — a full repo/blob copy pushed to a foreign host that
//!    would then SERVE PII, the §5.3 residency-boundary crossing) vs a **`CdnClone`** (within-EU
//!    ACCELERATION — a content-addressed, region-pinned clone/bundle, the Storage 11.2 class; no
//!    extra-EU edge serves PII). The classification is a property of the config, not the region.
//! 2. **[`OutboundMirrorGate`]** — the GA-11 decision over the EXISTING [`TransferGate`] policy:
//!    - a **within-EU CDN clone** is **ALLOWED** (within-EU acceleration is permitted, §5.3) — and a
//!      `CdnClone` whose target is EXTRA-EU is **DENIED** regardless (no extra-EU edge serves PII: a
//!      "CDN clone" is only an acceleration when it stays in-region);
//!    - a **PII-bearing push-mirror** to an **extra-EU** host is **DENIED BY DEFAULT** (the §5.2/§5.3
//!      deny-by-default — the resting state) UNLESS the EXISTING `transfer_allowed` records a lawful
//!      transfer mechanism for the target (the `[OPEN — LEGAL]` counsel-ratified entry);
//!    - a **within-EU push-mirror** (a different EU region) is **ALLOWED** via the policy (within-EU
//!      acceleration crosses no sovereignty boundary).
//!      Every deny bumps [`OutboundMirrorGate::extra_eu_pii_transfers_blocked`] — the GA-11 green
//!      artifact's value: **0 default extra-EU PII transfers** slip through (the count the drill reads).
//! 3. **A `non-PII-bearing` config carries no residency boundary** ([`OutboundConfig::pii_bearing`]):
//!    §5.3 is a gate for *PII-bearing* content; a config explicitly marked PII-free (e.g. a public,
//!    pseudonymised-already artifact) is not a transfer of personal data, so it is allowed (the gate
//!    is honest — it does not block a non-PII outbound config, only the PII-bearing crossing). The
//!    DEFAULT is PII-bearing (fail-closed: an unclassified config is treated as carrying PII).
//!
//! ## The ownership split is structural (§5.3 / EI-01 §7)
//! GDPR owns the **policy** the gate reads (this module + [`TransferGate`]); Tenancy/control-plane
//! owns **enforcement** (resolving the tenant's region of record + denying the push at the wire). This
//! module's [`OutboundMirrorGate::decide`] answers the lawful-transfer half — *"for a PII-bearing
//! outbound config of THIS kind to THIS region, is it permitted?"* — exactly the question the
//! control-plane `mirror_allowed` gate delegates to the GDPR `transfer_allowed` port. The CDC pair
//! wires this gate as the policy half of the control-plane decision (no production DAG edge — a
//! DEV-only test seam, the same edge `cdc_10_5_mirror_gate` uses).
//!
//! ## Floor named (VISION §3, recorded in writing)
//! The **residual** is named, not pretended-solved: an independent off-platform clone a third party
//! ALREADY HOLDS (a developer who cloned the repo to a laptop, a fork pushed elsewhere before the
//! gate) is **outside the platform's reach** — the gate prevents NEW extra-EU PII replication going
//! forward (0 default extra-EU PII transfers from the platform), but it cannot recall a copy a third
//! party already physically holds (§1 / §5.3 the hard residual). The counsel-ratified
//! `transfer_allowed` entries that would permit a SPECIFIC extra-EU mirror are **`[OPEN — LEGAL]`**
//! (Schrems II / GDPR Art. 44–49) — a parallel legal track, NOT an engineering gate; absent such a
//! recorded entry, this gate denies.
//!
//! ## Mutation floor (P-GA-36 TESTS — the deny-by-default + the within-EU-vs-extra-EU distinction are
//! mandatory-core). The behavioral core every mutation must be caught on:
//! [`OutboundMirrorGate::decide`] (the four-branch decision — CDN-clone-in-region allow,
//! CDN-clone-extra-EU deny, push-mirror-extra-EU deny-by-default, push-mirror-within-EU allow),
//! [`OutboundConfig::pii_bearing`] (the PII-free-config allow + the fail-closed default), and
//! [`OutboundMirrorGate::extra_eu_pii_transfers_blocked`] (the 0-default-extra-EU green-artifact
//! count). `cargo mutants -p myelin-gdpr-service -f
//! crates/myelin-gdpr-service/src/outbound_mirror_gate.rs` recorded in the commit body. **No
//! `--features integration` leg owed:** the gate is a pure decision over the already-shipped
//! [`TransferGate`] policy — it touches NO new DB / object-store / cache / bus contract (the
//! control-plane enforcement landing is Tenancy's wire, proven by the CDC pair; the within-EU CDN
//! clone blob class is Storage's, its own live-stack proof owned storage-side, P-255).

use myelin_tenancy::Region;

use crate::registries::{is_eea_region, TransferGate};

/// **The `outbound_mirror_pii_transfers_blocked` telemetry — the GA-11 green-artifact value.** The
/// count of PII-bearing outbound replication configs the gate DENIED (an extra-EU push-mirror without
/// a recorded mechanism, or a "CDN clone" that points extra-EU). The GA-11 gate reads this: **0
/// default extra-EU PII transfers** slip through (every PII-bearing extra-EU replication is caught).
/// PII-free: a count, never a payload.
pub const OUTBOUND_MIRROR_PII_TRANSFERS_BLOCKED: (&str, &str) =
    ("gdpr.outbound_mirror_pii_transfers_blocked", "count");

/// **The structural kind of an outbound replication config (§5.3 — the within-EU-acceleration vs
/// extra-EU-replication distinction).** This is the piece §5.3 makes first-class: the bare region
/// gate cannot by itself tell a within-EU CDN acceleration from an extra-EU replication, because the
/// *intent* differs even when a region matches. The kind is a property of the config the caller
/// declares, never inferred from the region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboundConfigKind {
    /// An outbound **push-mirror** — a full repo/blob REPLICATION pushed to a foreign host that would
    /// then SERVE the content (commit author identity, message bodies — PII-bearing). Targeting an
    /// extra-EU host is the §5.3 residency-boundary crossing: **denied by default**.
    PushMirror,
    /// A within-EU **CDN clone/bundle** — a content-addressed, region-pinned clone bundle (the Storage
    /// 11.2 class). It is an ACCELERATION, permitted §5.3 **only while it stays in-region**: no
    /// extra-EU edge serves PII. A CDN clone pointing extra-EU is NOT an acceleration — it is a
    /// disguised replication, and is denied.
    CdnClone,
}

/// **An outbound replication config the gate decides over (§5.3, PII-free).** The config's structural
/// kind ([`OutboundConfigKind`]), the region it targets, and whether it carries PII-bearing content.
/// PII-free by construction: a kind + a region code + a boolean — no principal, no payload (the gate
/// is a residency decision, never a payload path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundConfig {
    /// The structural kind — a replication (push-mirror) or an in-region acceleration (CDN clone).
    pub kind: OutboundConfigKind,
    /// The region the outbound config targets (the residency input — the far side of any crossing).
    pub target_region: Region,
    /// Whether the config carries PII-bearing content. The §5.3 gate is for PII-bearing content; a
    /// config explicitly marked PII-free is not a transfer of personal data. The DEFAULT (via
    /// [`OutboundConfig::push_mirror`] / [`OutboundConfig::cdn_clone`]) is **PII-bearing**
    /// (fail-closed — an unclassified config is treated as carrying PII).
    pub pii_bearing: bool,
}

impl OutboundConfig {
    /// A PII-bearing outbound **push-mirror** to `target_region` (the §5.3 replication — the
    /// fail-closed default is PII-bearing).
    pub fn push_mirror(target_region: Region) -> OutboundConfig {
        OutboundConfig {
            kind: OutboundConfigKind::PushMirror,
            target_region,
            pii_bearing: true,
        }
    }

    /// A PII-bearing within-EU **CDN clone/bundle** to `target_region` (the §5.3 / Storage 11.2
    /// acceleration — permitted only in-region; the fail-closed default is PII-bearing).
    pub fn cdn_clone(target_region: Region) -> OutboundConfig {
        OutboundConfig {
            kind: OutboundConfigKind::CdnClone,
            target_region,
            pii_bearing: true,
        }
    }

    /// Mark this config as explicitly **PII-free** (a public, already-pseudonymised artifact — not a
    /// transfer of personal data; the gate does not block it). The DEFAULT is PII-bearing; this is the
    /// honest carve-out for a config the caller attests carries no personal data.
    pub fn pii_free(mut self) -> OutboundConfig {
        self.pii_bearing = false;
        self
    }
}

/// **The verdict of the outbound push-mirror residency gate (§5.3, GA-11).** Either the outbound
/// config is **allowed** (a within-EU CDN acceleration; a within-EU push-mirror; an extra-EU
/// push-mirror WITH a recorded lawful transfer mechanism; or a PII-free config) or **denied** with a
/// loud reason (a PII-bearing extra-EU replication without a recorded mechanism; a "CDN clone"
/// pointing extra-EU). `Deny` is the resting state for a PII-bearing extra-EU crossing (deny-by-default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboundDecision {
    /// The outbound config is permitted. Carries the reason (observability — an allow is as auditable
    /// as a deny, §5.3 / EI-01 §3).
    Allow {
        /// Why the config was allowed (PII-free, for the audit trail).
        reason: OutboundAllowReason,
    },
    /// The outbound config is **refused** — a PII-bearing residency-boundary crossing WITHOUT a
    /// recorded lawful basis (the §5.3 deny-by-default), loud (never logged-and-allowed). A caller
    /// (the Git mirror feature / the control plane) that receives this MUST NOT replicate.
    Deny {
        /// Why the config was denied (PII-free — region code + the policy verdict, never a payload).
        reason: OutboundDenyReason,
    },
}

impl OutboundDecision {
    /// Whether the outbound config is allowed. A `false` is a HARD refusal the caller must honour — it
    /// must not replicate PII-bearing content when the gate denies.
    pub fn is_allowed(&self) -> bool {
        matches!(self, OutboundDecision::Allow { .. })
    }
}

/// **Why an outbound config was ALLOWED (§5.3, PII-free).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboundAllowReason {
    /// A within-EU **CDN clone/bundle** (the Storage 11.2 acceleration) — region-pinned within the
    /// EU/EEA, no extra-EU edge serves PII (§5.3 within-EU acceleration is permitted).
    WithinEuCdnAcceleration,
    /// A **push-mirror** whose target is within the EU/EEA — crosses no sovereignty boundary (within-EU
    /// acceleration via a mirror; the `transfer_allowed` region policy admits it structurally).
    WithinEuPushMirror,
    /// A **push-mirror** to an extra-EU target WITH a recorded lawful transfer mechanism in
    /// `transfer_allowed` (the `[OPEN — LEGAL]` counsel-ratified entry — the lawful-transfer path).
    LawfulExtraEuTransfer,
    /// The config is explicitly **PII-free** — not a transfer of personal data, so §5.3 does not gate
    /// it (the honest carve-out).
    NonPiiBearing,
}

/// **Why an outbound config was DENIED (§5.3, PII-free, loud).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboundDenyReason {
    /// A PII-bearing **push-mirror** to an extra-EU target with NO recorded lawful transfer mechanism —
    /// the §5.3 deny-by-default. The headline GA-11 refusal: an extra-EU PII-bearing replication is
    /// denied by default (0 default extra-EU PII transfers).
    ExtraEuReplicationDeniedByDefault {
        /// The extra-EU target region (the far side — the byte would land here; denied absent a basis).
        target_region: Region,
    },
    /// A **CDN clone** whose target is extra-EU — a "CDN clone" pointing extra-EU is not an
    /// acceleration (no extra-EU edge may serve PII, §5.3), so the gate refuses it: a CDN edge that
    /// served PII outside the EU would be a disguised replication.
    ExtraEuCdnEdgeDenied {
        /// The extra-EU target region the disguised-acceleration clone pointed at.
        target_region: Region,
    },
}

/// **The outbound push-mirror residency gate (§5.3, GA-11).** Decides over the EXISTING
/// [`TransferGate`] region-policy (deny extra-EU by default; the recorded-mechanism set), adding the
/// §5.3 **within-EU-acceleration-vs-extra-EU-replication distinction** the bare region gate cannot
/// express. Holds the running count of PII-bearing extra-EU transfers it blocked (the GA-11
/// green-artifact value). The gate borrows the [`TransferGate`] at decision time — it does not own or
/// duplicate it (EI-01 §7).
#[derive(Default)]
pub struct OutboundMirrorGate {
    /// The running count of PII-bearing extra-EU outbound replications the gate DENIED (every
    /// `Deny` — the §5.3 deny-by-default). The GA-11 drill asserts the number that *slipped through*
    /// is 0: this is the number the gate CAUGHT, and the caller honours the deny (the byte never
    /// leaves), so 0 default extra-EU PII transfers are made. PII-free scalar.
    extra_eu_pii_transfers_blocked: u64,
}

impl OutboundMirrorGate {
    /// A fresh gate — deny-by-default (no transfers blocked yet).
    pub fn new() -> OutboundMirrorGate {
        OutboundMirrorGate::default()
    }

    /// **`decide(config, &transfer_policy) → OutboundDecision` (§5.3, GA-11).**
    ///
    /// The decision, distinguishing within-EU acceleration from extra-EU replication:
    ///
    /// 1. A **PII-free** config is not a transfer of personal data — **allowed** (the honest
    ///    carve-out; the gate is for PII-bearing content).
    /// 2. A **CDN clone** ([`OutboundConfigKind::CdnClone`]): allowed ONLY if its target is within the
    ///    EU/EEA ([`is_eea_region`]) — a within-EU acceleration, no extra-EU edge serves PII. A CDN
    ///    clone pointing extra-EU is a disguised replication — **denied** ([`OutboundDenyReason::
    ///    ExtraEuCdnEdgeDenied`]).
    /// 3. A **push-mirror** ([`OutboundConfigKind::PushMirror`]): the residency-boundary decision over
    ///    the EXISTING [`TransferGate`] — a within-EU target is allowed structurally (within-EU
    ///    acceleration); an extra-EU target is **denied by default** UNLESS `transfer_allowed` records
    ///    a lawful mechanism for it (then allowed as a lawful transfer).
    ///
    /// Every PII-bearing deny bumps [`Self::extra_eu_pii_transfers_blocked`] (the GA-11 0-default zero).
    pub fn decide(
        &mut self,
        config: &OutboundConfig,
        transfer_policy: &TransferGate,
    ) -> OutboundDecision {
        // (1) A PII-free config is not a transfer of personal data — the §5.3 gate does not block it.
        if !config.pii_bearing {
            return OutboundDecision::Allow {
                reason: OutboundAllowReason::NonPiiBearing,
            };
        }

        match config.kind {
            // (2) A CDN clone is an acceleration ONLY while it stays in-region (no extra-EU edge serves
            //     PII, §5.3). An extra-EU "CDN clone" is a disguised replication — denied.
            OutboundConfigKind::CdnClone => {
                if is_eea_region(&config.target_region) {
                    OutboundDecision::Allow {
                        reason: OutboundAllowReason::WithinEuCdnAcceleration,
                    }
                } else {
                    self.extra_eu_pii_transfers_blocked += 1;
                    OutboundDecision::Deny {
                        reason: OutboundDenyReason::ExtraEuCdnEdgeDenied {
                            target_region: config.target_region.clone(),
                        },
                    }
                }
            }
            // (3) A push-mirror — the residency-boundary decision over the EXISTING transfer policy.
            OutboundConfigKind::PushMirror => {
                if is_eea_region(&config.target_region) {
                    // A within-EU push-mirror crosses no sovereignty boundary (within-EU acceleration).
                    OutboundDecision::Allow {
                        reason: OutboundAllowReason::WithinEuPushMirror,
                    }
                } else if transfer_policy
                    .transfer_allowed(&config.target_region)
                    .is_allowed()
                {
                    // An extra-EU target WITH a recorded lawful transfer mechanism — the lawful path.
                    OutboundDecision::Allow {
                        reason: OutboundAllowReason::LawfulExtraEuTransfer,
                    }
                } else {
                    // The §5.3 deny-by-default: an extra-EU PII-bearing replication without a basis.
                    self.extra_eu_pii_transfers_blocked += 1;
                    OutboundDecision::Deny {
                        reason: OutboundDenyReason::ExtraEuReplicationDeniedByDefault {
                            target_region: config.target_region.clone(),
                        },
                    }
                }
            }
        }
    }

    /// **The count of PII-bearing extra-EU outbound replications the gate BLOCKED** — the GA-11
    /// green-artifact value. The drill asserts the number that *slipped through* is **0**: this is the
    /// number the gate caught; a caller that honours the deny never replicates, so 0 default extra-EU
    /// PII transfers are made. PII-free scalar.
    pub fn extra_eu_pii_transfers_blocked(&self) -> u64 {
        self.extra_eu_pii_transfers_blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE HEADLINE (GA-11): a PII-bearing extra-EU push-mirror is DENIED BY DEFAULT; a within-EU
    /// CDN clone is ALLOWED.** The §5.3 within-EU-acceleration-vs-extra-EU-replication distinction, the
    /// dated green artifact's core: 0 default extra-EU PII transfers slip through.
    #[test]
    fn extra_eu_push_mirror_denied_within_eu_cdn_clone_allowed() {
        let policy = TransferGate::new(); // deny extra-EU by default (no recorded mechanisms).
        let mut gate = OutboundMirrorGate::new();

        // A PII-bearing push-mirror to an extra-EU host (us-east) — DENIED BY DEFAULT.
        let extra_eu_mirror = OutboundConfig::push_mirror(Region::new("us-east"));
        let denied = gate.decide(&extra_eu_mirror, &policy);
        assert_eq!(
            denied,
            OutboundDecision::Deny {
                reason: OutboundDenyReason::ExtraEuReplicationDeniedByDefault {
                    target_region: Region::new("us-east"),
                },
            },
            "a PII-bearing extra-EU push-mirror is denied by default (§5.3)"
        );
        assert!(
            !denied.is_allowed(),
            "the caller must NOT replicate on a Deny"
        );

        // A within-EU CDN clone (fr-par) — ALLOWED (within-EU acceleration, no extra-EU edge serves PII).
        let within_eu_clone = OutboundConfig::cdn_clone(Region::new("fr-par"));
        let allowed = gate.decide(&within_eu_clone, &policy);
        assert_eq!(
            allowed,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::WithinEuCdnAcceleration,
            },
            "a within-EU CDN clone is permitted (§5.3 acceleration)"
        );
        assert!(allowed.is_allowed());

        // The GA-11 green artifact: exactly the one extra-EU PII replication was blocked; the within-EU
        // clone allowed (not counted). 0 default extra-EU PII transfers slipped through.
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            1,
            "0 default extra-EU PII transfers slip through (the one mirror was blocked)"
        );
    }

    /// **A CDN clone pointing EXTRA-EU is a disguised replication — DENIED (§5.3: no extra-EU edge
    /// serves PII).** The kind alone does not exempt it; a "CDN clone" to us-east is refused.
    #[test]
    fn extra_eu_cdn_clone_is_a_disguised_replication_denied() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        let extra_eu_clone = OutboundConfig::cdn_clone(Region::new("us-east"));
        let decision = gate.decide(&extra_eu_clone, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Deny {
                reason: OutboundDenyReason::ExtraEuCdnEdgeDenied {
                    target_region: Region::new("us-east"),
                },
            },
            "a CDN clone pointing extra-EU is a disguised replication — denied (no extra-EU edge serves PII)"
        );
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            1,
            "the disguised extra-EU clone is counted as a blocked PII transfer"
        );
    }

    /// **A within-EU push-mirror (a different EU region) is ALLOWED via the policy (§5.3).** ACME
    /// (fr-par) mirrors to nl-ams: it crosses the tenant's region but stays within the EU/EEA — no
    /// sovereignty boundary, so allowed as a within-EU push-mirror.
    #[test]
    fn within_eu_cross_region_push_mirror_allowed() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        let within_eu_mirror = OutboundConfig::push_mirror(Region::new("nl-ams"));
        let decision = gate.decide(&within_eu_mirror, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::WithinEuPushMirror,
            },
            "a within-EU push-mirror crosses no sovereignty boundary (§5.3)"
        );
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            0,
            "a within-EU push-mirror is not a blocked extra-EU transfer"
        );
    }

    /// **An extra-EU push-mirror WITH a recorded lawful transfer mechanism flips to ALLOWED (§5.3).**
    /// The same us-east target denied by default is permitted once `transfer_allowed` records a
    /// mechanism — proving the gate CONSULTS the existing policy, it is not a blanket extra-EU block.
    #[test]
    fn extra_eu_push_mirror_allowed_only_with_recorded_mechanism() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();
        let mirror = OutboundConfig::push_mirror(Region::new("us-east"));

        // Before recording: denied by default.
        assert!(
            !gate.decide(&mirror, &policy).is_allowed(),
            "denied by default before a lawful basis is recorded"
        );
        assert_eq!(gate.extra_eu_pii_transfers_blocked(), 1);

        // Record the `[OPEN — LEGAL]` counsel-ratified entry on the EXISTING transfer policy.
        policy.record_transfer_mechanism(Region::new("us-east"));
        let decision = gate.decide(&mirror, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::LawfulExtraEuTransfer,
            },
            "an extra-EU push-mirror WITH a recorded transfer mechanism is permitted (the lawful path)"
        );
        // The allow did not bump the blocked counter — still the one deny before recording.
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            1,
            "the lawful-transfer allow did not bump the blocked count"
        );
    }

    /// **A PII-FREE outbound config is allowed — it is not a transfer of personal data (§5.3).** Even
    /// an extra-EU push-mirror is allowed when the config attests it carries no PII (the honest
    /// carve-out — the gate gates PII-bearing content, not every outbound byte).
    #[test]
    fn pii_free_config_is_allowed_even_extra_eu() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        let pii_free_mirror = OutboundConfig::push_mirror(Region::new("us-east")).pii_free();
        let decision = gate.decide(&pii_free_mirror, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::NonPiiBearing,
            },
            "a PII-free config is not a transfer of personal data — allowed even extra-EU"
        );
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            0,
            "a PII-free config is not a blocked PII transfer"
        );
    }

    /// **The default config is PII-bearing (fail-closed).** [`OutboundConfig::push_mirror`] /
    /// [`OutboundConfig::cdn_clone`] default to `pii_bearing = true` — an unclassified config is
    /// treated as carrying PII (the §5.3 fail-closed posture). Pins the default against a `-> false`
    /// mutant.
    #[test]
    fn the_default_config_is_pii_bearing_fail_closed() {
        assert!(
            OutboundConfig::push_mirror(Region::new("us-east")).pii_bearing,
            "a push-mirror defaults to PII-bearing (fail-closed)"
        );
        assert!(
            OutboundConfig::cdn_clone(Region::new("fr-par")).pii_bearing,
            "a CDN clone defaults to PII-bearing (fail-closed)"
        );
        assert!(
            !OutboundConfig::push_mirror(Region::new("us-east"))
                .pii_free()
                .pii_bearing,
            "pii_free() flips it off"
        );
    }

    /// **The blocked counter is a running total across mixed decisions (not a constant).** Two extra-EU
    /// PII replications + one allowed within-EU clone ⇒ the count is 2 (pins the accessor against a
    /// `-> 1` / `-> 0` mutant).
    #[test]
    fn blocked_counter_is_a_running_total() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        gate.decide(
            &OutboundConfig::push_mirror(Region::new("us-east")),
            &policy,
        ); // deny (+1)
        gate.decide(&OutboundConfig::cdn_clone(Region::new("ap-tokyo")), &policy); // deny (+1)
        gate.decide(&OutboundConfig::cdn_clone(Region::new("fr-par")), &policy); // allow (no bump)
        gate.decide(&OutboundConfig::push_mirror(Region::new("nl-ams")), &policy); // allow (no bump)

        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            2,
            "exactly the two extra-EU PII replications were blocked (the running total)"
        );
    }

    /// **The telemetry NAME/UNIT is anchored.**
    #[test]
    fn telemetry_signal_name_and_unit_are_anchored() {
        assert_eq!(
            OUTBOUND_MIRROR_PII_TRANSFERS_BLOCKED.0,
            "gdpr.outbound_mirror_pii_transfers_blocked"
        );
        assert_eq!(OUTBOUND_MIRROR_PII_TRANSFERS_BLOCKED.1, "count");
    }
}
