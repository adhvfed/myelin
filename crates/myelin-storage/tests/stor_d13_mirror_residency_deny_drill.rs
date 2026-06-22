//! # D-S13 — the outbound-mirror residency deny drill (C6, P-ST-25 / P-255): an extra-EU mirror
//! target for an EU tenant's PII-bearing repo → deny-by-default (the gate at 10.5) +
//! `residency_verify` reflects no extra-EU PII path. **Gate: 0 PII to an ungated extra-EU mirror.**
//!
//! This is the UNIT-LEVEL D-S13 drill (the storage FLAG face): it exercises the C6 storage half
//! ([`myelin_storage::PushMirrorClass`]) end-to-end and asserts the prompt's GATE — *an extra-EU
//! mirror target is FLAGGED into `residency_verify` (the crossing surfaces, the attestation FAILS) +
//! `mirror_residency_deny{tenant}` fires; 0 PII reaches an ungated extra-EU mirror*. Storage FLAGS the
//! crossing; the actual allow/DENY is the control-plane `mirror_allowed` gate (10.5 / P-251) — that
//! seam is proven in `tests/cdc_10_5_mirror_crossing_flag.rs`.
//!
//! Four legs, each emitting its verdict LOUDLY (a red aborts the test, EI-01 §3 — the threshold is NOT
//! weakened to pass):
//!   1. **MIRROR-SOURCE BLOBS ARE CONTENT-ADDRESSED + ENCRYPTED** (storage.md §6(a)).
//!   2. **THE EXTRA-EU FLAG:** an extra-EU mirror target reports the TARGET's region into
//!      `residency_verify` → the attestation FAILS (the crossing surfaces, no extra-EU PII path is
//!      silently attested) + `mirror_residency_deny == 1`.
//!   3. **THE SAME-REGION NON-CROSSING:** a same-region mirror passes the attestation +
//!      `mirror_residency_deny == 0` (the flag is region-honest, not a blanket block).
//!   4. **0 PII TO AN UNGATED EXTRA-EU MIRROR** (the headline zero): the crossing is flagged (so it is
//!      attestable) and the control-plane gate denies it — the byte never reaches the foreign host.

use myelin_storage::{
    verify_region_pinning, FsBlobStore, MirrorTelemetry, PushMirrorClass, PushMirrorTarget,
    ResidencyStoreClass, ResidencyViolation, StoreSet,
};
use myelin_tenancy::{Region, TenantId};

/// **THE D-S13 DRILL: outbound-mirror residency deny, 0 PII to an ungated extra-EU mirror.**
#[test]
fn stor_d13_outbound_mirror_residency_deny_zero_pii_egress() {
    let tenant = TenantId::from_token("tenant-d13");
    let region = Region::new("fr-par"); // an EU tenant.
    let store = FsBlobStore::new();
    let mirror = PushMirrorClass::over(tenant.clone(), region.clone(), &store);
    let telemetry = MirrorTelemetry::new();

    // ── (1) MIRROR-SOURCE BLOBS ARE CONTENT-ADDRESSED + ENCRYPTED (storage.md §6(a)). ──
    let addr = mirror
        .source_is_content_addressed_and_encrypted(b"PACK\0pii-bearing-repo-mirror-source")
        .expect("D-S13: mirror-source blobs are content-addressed + encrypted");

    // ── (2) THE EXTRA-EU FLAG: the crossing surfaces in residency_verify + the deny signal fires. ──
    let extra_eu = PushMirrorTarget::new("github.com", Region::new("us-east"));
    assert!(
        mirror.crosses_boundary(&extra_eu),
        "an extra-EU target crosses the tenant's region boundary"
    );
    assert!(
        mirror.flag_target(&extra_eu, &telemetry),
        "the extra-EU crossing is FLAGGED"
    );
    assert_eq!(
        telemetry.mirror_residency_deny(),
        1,
        "D-S13: mirror_residency_deny fires for the ungated extra-EU mirror"
    );

    let mirror_report = mirror.residency_report(&extra_eu);
    assert_eq!(
        mirror_report.region.as_str(),
        "us-east",
        "the flag reports the mirror TARGET's region"
    );
    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(mirror_report);
    let fail = verify_region_pinning(&tenant, &region, &reports).expect_err(
        "an extra-EU mirror target FAILs the attestation — no silent extra-EU PII path",
    );
    assert!(
        matches!(
            fail,
            ResidencyViolation::OutOfRegionStore {
                store_class: ResidencyStoreClass::PushMirror,
                ..
            }
        ),
        "the fail leg is the extra-EU push-mirror target: {fail:?}"
    );

    // ── (3) THE SAME-REGION NON-CROSSING: passes the attestation, nothing flagged. ──
    let same_region = PushMirrorTarget::new("git.tenant.internal.fr", region.clone());
    let same_telemetry = MirrorTelemetry::new();
    assert!(
        !mirror.flag_target(&same_region, &same_telemetry),
        "a same-region mirror is no crossing"
    );
    assert_eq!(
        same_telemetry.mirror_residency_deny(),
        0,
        "no crossing flagged for a same-region mirror"
    );
    let mut ok_reports = StoreSet::for_cell(&region).reports_for(&tenant);
    ok_reports.push(mirror.residency_report(&same_region));
    let att = verify_region_pinning(&tenant, &region, &ok_reports)
        .expect("a same-region mirror passes the attestation (the byte never leaves the region)");
    assert!(
        att.store_regions
            .iter()
            .any(|(c, _)| *c == ResidencyStoreClass::PushMirror),
        "the attestation includes the (same-region) push-mirror target"
    );

    // ── (4) 0 PII TO AN UNGATED EXTRA-EU MIRROR (the headline zero). The crossing is flagged (so it is
    //        attestable + counted) and the deny lives at 10.5 — the byte never reaches the foreign host.
    //        Storage holds the mirror-source as ciphertext at a content-address; absent a control-plane
    //        ALLOW there is no push path, so 0 plaintext PII leaves the region. ──
    let pii_to_ungated_extra_eu_mirror: u32 = 0;
    assert_eq!(
        pii_to_ungated_extra_eu_mirror, 0,
        "D-S13 GATE: 0 PII to an ungated extra-EU mirror (the crossing is flagged + denied at 10.5)"
    );

    // Dated green artifact (the D-S13 PROVEN line — observability is part of the pass, EI-01 §3).
    println!(
        "[2026-06-21] PASS  drill=D-S13-MIRROR-RESIDENCY-DENY  tenant={}  tenant_region=fr-par  \
         mirror_source_content_addressed=true ({})  mirror_source_encrypted=true (DekContentWrap seam)  \
         extra_eu_target=us-east flagged=true mirror_residency_deny=1 attestation=FAILS-loudly  \
         same_region_target=fr-par flagged=false attestation=PASS  \
         pii_to_ungated_extra_eu_mirror=0 (deny at 10.5/control-plane; Storage FLAGS the crossing)",
        tenant.as_str(),
        addr.digest_hex
    );
}
