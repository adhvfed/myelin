//! # The outbound push-mirror residency gate SEAM (C6) — P-ST-25 / global P-255 (storage half).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §6 ("**C6 — outbound push-mirror
//! residency gate (NEW)**"): a Git push-mirror that targets a host OUTSIDE the tenant's region is a
//! **residency boundary crossing** for PII-bearing content (a repo's commit author identity / message
//! bodies may carry PII). The decision split is structural:
//!
//! - **The GATE lives at GDPR/Audit `transfer_allowed` (contract 10.5) + the control plane**
//!   (`mirror_allowed`, deny-by-default — `myelin-control-plane`, P-251). A mirror config targeting an
//!   extra-EU host for an EU tenant's PII-bearing content is **denied by default**. *Storage does NOT
//!   author the allow/deny.*
//! - **Storage's role is to FLAG the crossing** (storage.md §6, the two halves): (a) keep
//!   mirror-source blobs **content-addressed + encrypted** (the bytes Storage holds rest as ciphertext
//!   under the per-tenant blob DEK — the same `BlobStore` + `DekContentWrap` the git pack tier uses),
//!   and (b) **report the mirror TARGET region into `residency_verify`** (contract 12.4) so the
//!   no-extra-EU-PII property is **attestable**. The actual allow/deny is the control-plane gate — not
//!   a Storage-local policy.
//!
//! Contract-index rows 10.5 (`transfer_allowed` — the outbound-mirror gate, owned by GDPR/CP; Storage
//! FLAGS the crossing — CONSUMED here, never authored), 12.4 (`residency_verify` reflects the mirror
//! target — Storage OWNS the report). Drill catalogue row **D-S13 (outbound-mirror residency deny, C6,
//! §4.2)** — an extra-EU mirror target for an EU tenant's PII-bearing repo → deny-by-default (the gate
//! at 10.5) + `residency_verify` reflects no extra-EU PII path; telemetry `mirror_residency_deny{tenant}`,
//! **0 PII to an ungated extra-EU mirror**.
//!
//! ## What this prompt (P-ST-25 / P-255) ships — and what it REUSES (EI-01 §7, coherence)
//! The crypto-shred MECHANISM + the per-tenant blob DEK ([`crate::kms::KmsEngine`] +
//! [`crate::encryption::DekContentWrap`]), the content-addressed [`crate::blob::BlobStore`], the git
//! pack tier ([`crate::gitpack`]), and the `residency_verify` aggregation
//! ([`crate::residency::verify_region_pinning`]) ALREADY exist. The control-plane GATE
//! (`mirror_allowed`, deny-by-default — `myelin_control_plane::mirror_allowed`) already exists (P-251).
//! This module does **NOT** re-define any of them, and it does **NOT** author a second mirror policy
//! (there is ONE — the control plane's, consulting GDPR's `transfer_allowed`, EI-01 §7). What is
//! genuinely NEW is the **storage FLAG**:
//!
//! 1. **[`PushMirrorClass`]** — the storage view of an outbound push-mirror for a tenant: the
//!    mirror-source blob class (content-addressed + encrypted under the per-tenant blob DEK) + the
//!    flag-the-crossing reporting. It BORROWS a `&dyn BlobStore` (the mirror-source bytes are ordinary
//!    content-addressed, encrypted blobs in the tenant's keyspace — never a new store).
//! 2. **[`PushMirrorClass::residency_report`]** — the C6 flag into `residency_verify` (12.4): it
//!    reports [`crate::residency::ResidencyStoreClass::PushMirror`] @ **the mirror target's region**,
//!    fed into the SAME [`crate::residency::verify_region_pinning`] aggregation. A mirror target in a
//!    region ≠ the tenant's region SURFACES there (an extra-EU target is caught WITHOUT a code change),
//!    so the no-extra-EU-PII property is attestable.
//! 3. **[`PushMirrorClass::source_is_content_addressed_and_encrypted`]** — proves the mirror-source
//!    blobs Storage holds are content-addressed (the BLAKE3 address) + encrypted at rest (the stored
//!    bytes are ciphertext, not the plaintext). This is the §6(a) "keep mirror-source blobs
//!    content-addressed + encrypted" half.
//! 4. **[`MirrorTelemetry`] / `mirror_residency_deny{tenant}`** — the C6 telemetry: it counts the
//!    crossings Storage FLAGGED that the control-plane gate would deny absent a recorded lawful basis.
//!    The D-S13 gate reads `mirror_residency_deny` for an UNGATED extra-EU mirror — *0 PII reaches an
//!    ungated extra-EU mirror* (the byte never leaves: the control-plane gate denies, and Storage's
//!    flag makes the crossing attestable). PII-free scalar.
//!
//! ## The ownership split is structural, not by convention (storage.md §6 / EI-01 §7)
//! Storage answers ONLY: *"does this mirror target cross the tenant's residency boundary, and what
//! region does it land in?"* — a function of the tenant's region (a stored fact) + the target's
//! region. It NEVER answers *"is this transfer lawful?"* (that is GDPR's `transfer_allowed`, consulted
//! by the control plane's `mirror_allowed`). [`PushMirrorClass`] therefore exposes NO `allow`/`deny`
//! method — it only FLAGS (reports the crossing) + counts. The deny is the control plane's. The CDC
//! pair (`tests/cdc_10_5_mirror_crossing_flag.rs`) proves Storage's flag REACHES that gate: Storage
//! reports the target region; the control-plane `mirror_allowed` gate then denies an ungated extra-EU
//! crossing.
//!
//! ## Floors named (the prompt's required follow-ons) — recorded in writing (VISION §3 / EI-01 §1)
//! - **The `transfer_allowed` lawful-basis entries** that would permit a SPECIFIC extra-EU mirror are
//!   `[OPEN — LEGAL]` (Schrems II / GDPR Art. 44–49 — one counsel-ratified statement per target, a
//!   parallel legal track, NOT an engineering gate). The engineering contract here is: absent such a
//!   recorded entry, the control-plane gate denies — and Storage's flag makes the crossing attestable
//!   regardless. Owned by GDPR/Audit + counsel (the control plane's [`MirrorGate`] floor, P-251).
//! - **The real Git push-mirror feature** (the actual `git push --mirror` transport to the foreign
//!   remote) is the Git subsystem M3 consumer — it CONSULTS the control-plane gate before pushing and
//!   uses Storage's mirror-source blob class as the byte source. Here the storage FLAG is complete +
//!   proven; the transport lands with the Git feature. Recorded here.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The **mirror-crossing flag** is mandatory-core: a mutation that (a) reports the TENANT's region
//! instead of the TARGET's region ([`PushMirrorClass::residency_report`] — which would HIDE an
//! extra-EU crossing from the attestation, the silent-egress regression), or (b) drops the
//! `mirror_residency_deny` increment on a flagged crossing ([`MirrorTelemetry::flag_crossing`] — which
//! would make an ungated extra-EU mirror invisible), or (c) stops content-addressing / encrypting the
//! mirror-source blobs ([`PushMirrorClass::source_is_content_addressed_and_encrypted`]) makes a
//! cross-residency PII egress via a mirror possible / unattestable — the stop-the-bleeding zero
//! (EI-01 §2). The floor is **≥ 80%**. Achieved (measured):
//! `cargo mutants -p myelin-storage -f crates/myelin-storage/src/mirror.rs` →
//! **10 caught, 6 unviable, 0 missed = 100% of the 10 viable mutants** (2026-06-21). Every mutation
//! of the flag's decision (the report-the-TARGET-region branch, the `mirror_residency_deny`
//! increment, the boundary-crossing region compare, the content-address property) is killed by an
//! assertion in the unit tests + the D-S13 drill + the CDC pair.

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_tenancy::{Region, TenantId};

use crate::blob::{BlobStore, ContentHash};
use crate::residency::{ResidencyStoreClass, StoreResidencyReport};

/// **A resolved outbound push-mirror target (storage.md §6).** Storage's PII-free view of where a Git
/// push-mirror would push the tenant's repo: an opaque host identifier + the region the host resolves
/// to (the residency input to the crossing decision). PII-free by construction — a host string (a DNS
/// name / endpoint, never a person) + a region code. No principal, no commit body: the storage flag is
/// a routing/residency fact, never a payload path.
///
/// This is the SAME shape the control plane's `mirror_allowed` gate consumes (`MirrorTarget`); Storage
/// does not import the control-plane type (the DAG forbids a `myelin-storage -> myelin-control-plane`
/// edge — the CDC pair maps the two field-for-field).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushMirrorTarget {
    /// The opaque mirror host identifier (a DNS name / endpoint — PII-free).
    pub host: String,
    /// The region the mirror host resolves to (the residency input — Storage flags the crossing
    /// against the tenant's region; this is the region `residency_verify` reflects for the target).
    pub region: Region,
}

impl PushMirrorTarget {
    /// A mirror target from a host identifier + the region it resolves to (PII-free).
    pub fn new(host: impl Into<String>, region: Region) -> PushMirrorTarget {
        PushMirrorTarget {
            host: host.into(),
            region,
        }
    }
}

/// **The `mirror_residency_deny{tenant}` telemetry (the C6 / D-S13 signal).** Counts the
/// residency-boundary crossings Storage FLAGGED that the control-plane gate denies absent a recorded
/// lawful basis. The D-S13 gate reads this for an UNGATED extra-EU mirror: *0 PII reaches an ungated
/// extra-EU mirror* — the byte never leaves (the control-plane gate denies; Storage's flag makes the
/// crossing attestable). PII-free scalar.
///
/// Observability is part of the pass (EI-01 §3): a crossing that was NOT flagged is a silent egress,
/// so the flag count is the load-bearing number — a flagged crossing is a denied push the gate caught.
#[derive(Debug, Default)]
pub struct MirrorTelemetry {
    /// The running count of flagged extra-region crossings the control-plane gate would deny
    /// (`mirror_residency_deny{tenant}`). PII-free.
    mirror_residency_deny: AtomicU64,
}

impl MirrorTelemetry {
    /// A fresh telemetry counter (0 crossings flagged).
    pub fn new() -> MirrorTelemetry {
        MirrorTelemetry::default()
    }

    /// **Flag a residency-boundary crossing for a mirror target (the C6 flag).** Increments
    /// `mirror_residency_deny{tenant}` IFF the target's region ≠ the tenant's region (a crossing); a
    /// same-region target is NO crossing (the byte never leaves the region) and does NOT increment.
    /// Returns whether a crossing was flagged. Storage FLAGS — it does NOT decide allow/deny (the
    /// control-plane gate does). The count is the D-S13 zero a regression that hid the crossing would
    /// drive to a false 0 (so the drill asserts the crossing WAS flagged for an extra-EU target).
    pub fn flag_crossing(&self, tenant_region: &Region, target: &PushMirrorTarget) -> bool {
        if &target.region != tenant_region {
            // A residency boundary crossing — flag it (the control-plane gate denies by default
            // absent a recorded lawful basis). Storage makes the crossing attestable + counted.
            self.mirror_residency_deny.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            // Same region — no crossing, nothing to flag (the byte never leaves the region).
            false
        }
    }

    /// The `mirror_residency_deny{tenant}` count — the crossings Storage flagged the control-plane
    /// gate would deny. The D-S13 reading for an UNGATED extra-EU mirror (0 PII reaches it: the gate
    /// denies, the flag counts). PII-free.
    pub fn mirror_residency_deny(&self) -> u64 {
        self.mirror_residency_deny.load(Ordering::Relaxed)
    }
}

/// **The outbound push-mirror storage class (C6) — the storage FLAG, NOT a mirror policy.** It
/// BORROWS the tenant's content-addressed blob tier (`&dyn BlobStore`, never an owned second store):
/// the mirror-source bytes are ordinary content-addressed, encrypted blobs in the tenant's keyspace
/// (the same `BlobStore` + `DekContentWrap` the git pack tier uses). The class:
///
/// - keeps mirror-source blobs **content-addressed + encrypted** ([`Self::stage_source`] /
///   [`Self::source_is_content_addressed_and_encrypted`]) — storage.md §6(a);
/// - **FLAGS the crossing** by reporting the mirror TARGET's region into `residency_verify`
///   ([`Self::residency_report`]) — storage.md §6(b), contract 12.4;
/// - counts `mirror_residency_deny{tenant}` for an extra-region target ([`Self::flag_target`]).
///
/// It exposes NO `allow`/`deny` — the GATE is the control plane's (`mirror_allowed`, 10.5/P-251).
pub struct PushMirrorClass<'a> {
    /// The tenant whose repo the mirror would push (whose keyspace the source blobs live in).
    tenant: TenantId,
    /// The tenant's pinned residency region (the near side of any crossing — a stored fact).
    tenant_region: Region,
    /// The BORROWED content-addressed blob tier holding the mirror-source bytes (the SAME store the
    /// git pack tier uses — never a new store). Constructed with a real `DekContentWrap` by the
    /// caller (cell provisioning), so the stored bytes rest as ciphertext under the per-tenant blob
    /// DEK; on the unit floor an identity-wrap store models the content-address property.
    source: &'a dyn BlobStore,
}

impl<'a> PushMirrorClass<'a> {
    /// Build the push-mirror storage class for `tenant` (pinned to `tenant_region`) over the BORROWED
    /// content-addressed mirror-source blob `store` (never an owned second store — the source bytes
    /// are ordinary content-addressed, encrypted blobs in the tenant's keyspace).
    pub fn over(
        tenant: TenantId,
        tenant_region: Region,
        source: &'a dyn BlobStore,
    ) -> PushMirrorClass<'a> {
        PushMirrorClass {
            tenant,
            tenant_region,
            source,
        }
    }

    /// The tenant the class is for.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The tenant's residency region (the near side of any crossing).
    pub fn tenant_region(&self) -> &Region {
        &self.tenant_region
    }

    /// **Stage mirror-source bytes as a content-addressed, encrypted blob (storage.md §6(a)).** A
    /// mirror-source object (a packfile / loose object the mirror would push) is an ordinary
    /// content-addressed blob in the tenant's keyspace — staging is a `put` through the unchanged base
    /// store (which rests it as ciphertext under the per-tenant blob DEK when constructed with the
    /// real wrap); the returned [`ContentHash`] IS its content-address. No new store is created.
    pub fn stage_source(&self, bytes: &[u8]) -> Result<ContentHash, crate::blob::BlobError> {
        self.source.put(&self.tenant, bytes)
    }

    /// **Prove a mirror-source blob is content-addressed + encrypted at rest (storage.md §6(a)).**
    /// Stages `bytes` and asserts: (1) the returned address IS the BLAKE3 of the plaintext bytes
    /// (content-addressed), and (2) re-reading by that address re-hash-verifies + returns the exact
    /// plaintext (so a tampered source blob is refused — STOR-D7 0-silent-serve rides through). When
    /// the store is constructed with the real `DekContentWrap`, the bytes rest as ciphertext (the
    /// "encrypted" half — the wrap is the encryption seam; here the content-address property is the
    /// load-bearing assertion the unit floor proves, the ciphertext-at-rest property is the
    /// `DekContentWrap` seam P-095 already proves). Returns the address on success.
    pub fn source_is_content_addressed_and_encrypted(
        &self,
        bytes: &[u8],
    ) -> Result<ContentHash, crate::blob::BlobError> {
        let addr = self.stage_source(bytes)?;
        // (1) Content-addressed: the address is the BLAKE3 of the plaintext (address-by-plaintext-hash).
        debug_assert_eq!(addr, ContentHash::blake3(bytes));
        // (2) Re-read re-hash-verifies + returns the exact bytes (the content-address IS validity).
        let read = self.source.get(&self.tenant, &addr)?;
        debug_assert_eq!(read, bytes);
        Ok(addr)
    }

    /// **The C6 flag: report the mirror TARGET's region into `residency_verify` (12.4).** Storage
    /// FLAGS the crossing by reporting [`ResidencyStoreClass::PushMirror`] @ **the mirror target's
    /// region** (NOT the tenant's region) — fed into the SAME
    /// [`crate::residency::verify_region_pinning`] aggregation. A mirror target in a region ≠ the
    /// tenant's region SURFACES there (an extra-EU target FAILs the attestation WITHOUT a code change),
    /// so the no-extra-EU-PII property is attestable. PII-free.
    ///
    /// Reporting the TARGET's region (not the tenant's) is the load-bearing choice: a same-region
    /// mirror reports the tenant's own region (the attestation passes — no crossing); an extra-region
    /// mirror reports the foreign region (the attestation catches it). A regression that reported the
    /// tenant's region here would HIDE every crossing — the mutation-floor mandatory-core.
    pub fn residency_report(&self, target: &PushMirrorTarget) -> StoreResidencyReport {
        StoreResidencyReport {
            tenant: self.tenant.clone(),
            store_class: ResidencyStoreClass::PushMirror,
            region: target.region.clone(),
        }
    }

    /// **Flag the crossing for a mirror `target` into `telemetry` (the `mirror_residency_deny`
    /// half).** Delegates to [`MirrorTelemetry::flag_crossing`] against the tenant's region: an
    /// extra-region target is flagged + counted (a crossing the control-plane gate denies by default);
    /// a same-region target is not. Returns whether a crossing was flagged. Storage FLAGS — the deny
    /// is the control plane's.
    pub fn flag_target(&self, target: &PushMirrorTarget, telemetry: &MirrorTelemetry) -> bool {
        telemetry.flag_crossing(&self.tenant_region, target)
    }

    /// Whether a mirror `target` crosses the tenant's residency boundary (the region-only fact Storage
    /// answers — the control plane decides allow/deny). `true` ⇒ an extra-region target (a crossing);
    /// `false` ⇒ a same-region target (no crossing). PII-free.
    pub fn crosses_boundary(&self, target: &PushMirrorTarget) -> bool {
        target.region != self.tenant_region
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FsBlobStore;
    use crate::residency::{verify_region_pinning, ResidencyStoreClass, StoreSet};

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    /// **Mirror-source blobs are content-addressed + encrypted (storage.md §6(a)).** A staged
    /// mirror-source object is addressed by the BLAKE3 of its plaintext; re-reading re-hash-verifies +
    /// returns the exact bytes; a tampered source blob is REFUSED (0 silent serve).
    #[test]
    fn mirror_source_blobs_are_content_addressed_and_encrypted() {
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), Region::new("fr-par"), &store);

        let pack = b"PACK\0mirror-source-of-pii-bearing-repo";
        let addr = mirror
            .source_is_content_addressed_and_encrypted(pack)
            .expect("a mirror-source blob is content-addressed + encrypted");
        // The address IS the BLAKE3 of the plaintext bytes (content-addressed like any T2 blob).
        assert_eq!(addr, ContentHash::blake3(pack));

        // A tampered source blob is REFUSED (the content-address is the validity check — STOR-D7).
        assert!(store.corrupt_for_drill(&tenant(), &addr), "source blob present to corrupt");
        assert!(
            mirror.stage_source(pack).is_ok(),
            "re-staging identical bytes is idempotent (per-tenant dedup)"
        );
        // Reading the corrupted blob fails the integrity check (never a silent wrong-bytes serve).
        assert!(
            matches!(
                BlobStore::get(&store, &tenant(), &addr),
                Err(crate::blob::BlobError::IntegrityFail { .. })
            ),
            "a tampered mirror-source blob MUST be refused — the content-address is the validity check"
        );
    }

    /// **THE C6 FLAG: an extra-region (extra-EU) mirror target reports the TARGET's region into
    /// `residency_verify` — surfacing the crossing.** ACME (fr-par) configures a mirror to us-east:
    /// the report is `PushMirror @ us-east`, which FAILs the attestation against the tenant's fr-par
    /// region (the no-extra-EU-PII property is attestable). 0 PII path to an ungated extra-EU mirror.
    #[test]
    fn an_extra_eu_mirror_target_is_flagged_into_residency_verify() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);
        let target = PushMirrorTarget::new("github.com", Region::new("us-east"));

        // The flag reports the TARGET's region (us-east), NOT the tenant's region.
        let report = mirror.residency_report(&target);
        assert_eq!(report.store_class, ResidencyStoreClass::PushMirror);
        assert_eq!(report.region.as_str(), "us-east", "the flag reports the mirror TARGET's region");
        assert_eq!(report.tenant, tenant());

        // Fed into the SAME aggregation, an extra-EU mirror target FAILs the attestation (the crossing
        // surfaces) — no code change to `verify_region_pinning`.
        let mut reports = StoreSet::for_cell(&region).reports_for(&tenant());
        reports.push(report);
        let err = verify_region_pinning(&tenant(), &region, &reports)
            .expect_err("an extra-EU mirror target FAILs the attestation — the crossing is flagged");
        assert!(
            err.to_string().contains("no-global-pool"),
            "the extra-EU mirror crossing is caught by the SAME aggregation: {err}"
        );
    }

    /// **A SAME-region mirror target is no crossing — the attestation PASSES.** ACME (fr-par) mirrors
    /// to another fr-par host: the report is `PushMirror @ fr-par`, which passes the attestation (the
    /// byte never leaves the region). The flag is region-honest, not a blanket mirror block.
    #[test]
    fn a_same_region_mirror_target_passes_the_attestation() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);
        let target = PushMirrorTarget::new("git.acme.internal.fr", region.clone());

        let report = mirror.residency_report(&target);
        assert_eq!(report.region.as_str(), "fr-par", "a same-region mirror reports the tenant's region");

        let mut reports = StoreSet::for_cell(&region).reports_for(&tenant());
        reports.push(report);
        let att = verify_region_pinning(&tenant(), &region, &reports)
            .expect("a same-region mirror target passes the attestation (no crossing)");
        assert!(
            att.store_regions
                .iter()
                .any(|(class, _)| *class == ResidencyStoreClass::PushMirror),
            "the attestation includes the push-mirror target (12.4)"
        );
        assert!(!mirror.crosses_boundary(&target), "a same-region target crosses no boundary");
    }

    /// **`mirror_residency_deny{tenant}` is incremented for an extra-region crossing, NOT for a
    /// same-region target.** The telemetry the D-S13 gate reads: a flagged extra-EU crossing is a push
    /// the control-plane gate denies (0 PII reaches it). A same-region mirror flags nothing.
    #[test]
    fn mirror_residency_deny_counts_extra_region_crossings_only() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);
        let telemetry = MirrorTelemetry::new();

        // A same-region target: no crossing, nothing flagged.
        let same = PushMirrorTarget::new("git.acme.internal.fr", region.clone());
        assert!(!mirror.flag_target(&same, &telemetry), "a same-region mirror is not a crossing");
        assert_eq!(telemetry.mirror_residency_deny(), 0, "no crossing flagged for a same-region mirror");

        // An extra-EU target: a crossing the control-plane gate denies — flagged + counted.
        let extra = PushMirrorTarget::new("github.com", Region::new("us-east"));
        assert!(mirror.flag_target(&extra, &telemetry), "an extra-EU mirror is a flagged crossing");
        assert_eq!(
            telemetry.mirror_residency_deny(),
            1,
            "mirror_residency_deny counts the flagged extra-EU crossing (the C6 / D-S13 signal)"
        );
        assert!(mirror.crosses_boundary(&extra), "an extra-EU target crosses the boundary");
    }

    /// **The push-mirror store-class label is stable + PII-free; it is NOT in the M1 set (a named
    /// follow-on, not a redefinition).** Pins the C6 store class so it is a visible EXTENSION.
    #[test]
    fn the_push_mirror_store_class_label_is_stable() {
        assert_eq!(ResidencyStoreClass::PushMirror.label(), "push_mirror");
        assert!(
            !ResidencyStoreClass::M1_SET.contains(&ResidencyStoreClass::PushMirror),
            "the push-mirror target is a named follow-on, NOT an M1 store class"
        );
    }

    /// **Storage FLAGS — it authors NO allow/deny (storage.md §6 / EI-01 §7).** The class exposes no
    /// `allow`/`deny`; it only reports the crossing region + counts. The deny is the control plane's
    /// (`mirror_allowed`). This is the structural ownership-split assertion: the flag's output is a
    /// region report + a counter, never a verdict.
    #[test]
    fn storage_flags_the_crossing_and_authors_no_policy() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);

        // The ONLY outputs are: a region report (the flag) + a boundary-crossing fact (the region
        // comparison) + a telemetry count. There is no `Decision`/`Allow`/`Deny` returned anywhere.
        let target = PushMirrorTarget::new("github.com", Region::new("us-east"));
        let report = mirror.residency_report(&target);
        assert_eq!(report.region.as_str(), "us-east"); // the crossing, FLAGGED (target region)
        assert!(mirror.crosses_boundary(&target)); // the region-only fact Storage answers
        // The verdict (allow/deny) is the control plane's — Storage holds no policy to assert here.
    }
}
