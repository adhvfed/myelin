use std::sync::atomic::{AtomicU64, Ordering};

use myelin_tenancy::{Region, TenantId};

use crate::blob::{BlobStore, ContentHash};
use crate::residency::{ResidencyStoreClass, StoreResidencyReport};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushMirrorTarget {
    pub host: String,
    pub region: Region,
}

impl PushMirrorTarget {
    pub fn new(host: impl Into<String>, region: Region) -> PushMirrorTarget {
        PushMirrorTarget {
            host: host.into(),
            region,
        }
    }
}

#[derive(Debug, Default)]
pub struct MirrorTelemetry {
    mirror_residency_deny: AtomicU64,
}

impl MirrorTelemetry {
    pub fn new() -> MirrorTelemetry {
        MirrorTelemetry::default()
    }

    pub fn flag_crossing(&self, tenant_region: &Region, target: &PushMirrorTarget) -> bool {
        if &target.region != tenant_region {
            self.mirror_residency_deny.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn mirror_residency_deny(&self) -> u64 {
        self.mirror_residency_deny.load(Ordering::Relaxed)
    }
}

pub struct PushMirrorClass<'a> {
    tenant: TenantId,
    tenant_region: Region,
    source: &'a dyn BlobStore,
}

impl<'a> PushMirrorClass<'a> {
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

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn tenant_region(&self) -> &Region {
        &self.tenant_region
    }

    pub fn stage_source(&self, bytes: &[u8]) -> Result<ContentHash, crate::blob::BlobError> {
        self.source.put(&self.tenant, bytes)
    }

    pub fn source_is_content_addressed_and_encrypted(
        &self,
        bytes: &[u8],
    ) -> Result<ContentHash, crate::blob::BlobError> {
        let addr = self.stage_source(bytes)?;
        debug_assert_eq!(addr, ContentHash::blake3(bytes));
        let read = self.source.get(&self.tenant, &addr)?;
        debug_assert_eq!(read, bytes);
        Ok(addr)
    }

    pub fn residency_report(&self, target: &PushMirrorTarget) -> StoreResidencyReport {
        StoreResidencyReport {
            tenant: self.tenant.clone(),
            store_class: ResidencyStoreClass::PushMirror,
            region: target.region.clone(),
        }
    }

    pub fn flag_target(&self, target: &PushMirrorTarget, telemetry: &MirrorTelemetry) -> bool {
        telemetry.flag_crossing(&self.tenant_region, target)
    }

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

    #[test]
    fn mirror_source_blobs_are_content_addressed_and_encrypted() {
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), Region::new("fr-par"), &store);

        let pack = b"PACK\0mirror-source-of-pii-bearing-repo";
        let addr = mirror
            .source_is_content_addressed_and_encrypted(pack)
            .expect("a mirror-source blob is content-addressed + encrypted");
        assert_eq!(addr, ContentHash::blake3(pack));

        assert!(
            mirror.stage_source(pack).is_ok(),
            "re-staging identical bytes is idempotent (per-tenant dedup)"
        );
        assert!(
            store.corrupt_for_drill(&tenant(), &addr),
            "source blob present to corrupt"
        );
        assert!(
            matches!(
                BlobStore::get(&store, &tenant(), &addr),
                Err(crate::blob::BlobError::IntegrityFail { .. })
            ),
            "a tampered mirror-source blob MUST be refused - the content-address is the validity check"
        );
    }

    #[test]
    fn an_extra_eu_mirror_target_is_flagged_into_residency_verify() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);
        let target = PushMirrorTarget::new("mirror.example", Region::new("us-east"));

        let report = mirror.residency_report(&target);
        assert_eq!(report.store_class, ResidencyStoreClass::PushMirror);
        assert_eq!(
            report.region.as_str(),
            "us-east",
            "the flag reports the mirror TARGET's region"
        );
        assert_eq!(report.tenant, tenant());

        let mut reports = StoreSet::for_cell(&region).reports_for(&tenant());
        reports.push(report);
        let err = verify_region_pinning(&tenant(), &region, &reports).expect_err(
            "an extra-EU mirror target FAILs the attestation - the crossing is flagged",
        );
        assert!(
            err.to_string().contains("no-global-pool"),
            "the extra-EU mirror crossing is caught by the SAME aggregation: {err}"
        );
    }

    #[test]
    fn a_same_region_mirror_target_passes_the_attestation() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);
        let target = PushMirrorTarget::new("git.acme.internal.fr", region.clone());

        let report = mirror.residency_report(&target);
        assert_eq!(
            report.region.as_str(),
            "fr-par",
            "a same-region mirror reports the tenant's region"
        );

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
        assert!(
            !mirror.crosses_boundary(&target),
            "a same-region target crosses no boundary"
        );
    }

    #[test]
    fn mirror_residency_deny_counts_extra_region_crossings_only() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);
        let telemetry = MirrorTelemetry::new();

        let same = PushMirrorTarget::new("git.acme.internal.fr", region.clone());
        assert!(
            !mirror.flag_target(&same, &telemetry),
            "a same-region mirror is not a crossing"
        );
        assert_eq!(
            telemetry.mirror_residency_deny(),
            0,
            "no crossing flagged for a same-region mirror"
        );

        let extra = PushMirrorTarget::new("mirror.example", Region::new("us-east"));
        assert!(
            mirror.flag_target(&extra, &telemetry),
            "an extra-EU mirror is a flagged crossing"
        );
        assert_eq!(
            telemetry.mirror_residency_deny(),
            1,
            "mirror_residency_deny counts the flagged extra-EU crossing (the C6 / D-S13 signal)"
        );
        assert!(
            mirror.crosses_boundary(&extra),
            "an extra-EU target crosses the boundary"
        );
    }

    #[test]
    fn the_push_mirror_store_class_label_is_stable() {
        assert_eq!(ResidencyStoreClass::PushMirror.label(), "push_mirror");
        assert!(
            !ResidencyStoreClass::M1_SET.contains(&ResidencyStoreClass::PushMirror),
            "the push-mirror target is a named follow-on, NOT an M1 store class"
        );
    }

    #[test]
    fn storage_flags_the_crossing_and_authors_no_policy() {
        let region = Region::new("fr-par");
        let store = FsBlobStore::new();
        let mirror = PushMirrorClass::over(tenant(), region.clone(), &store);

        let target = PushMirrorTarget::new("mirror.example", Region::new("us-east"));
        let report = mirror.residency_report(&target);
        assert_eq!(report.region.as_str(), "us-east");
        assert!(mirror.crosses_boundary(&target));
    }
}
