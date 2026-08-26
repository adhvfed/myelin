use myelin_storage::{KmsError, PiiKeyRef, NONCE_LEN};
use myelin_tenancy::{Region, ResidencyTag, TenantId};

use crate::dek::SearchDekPin;
use crate::store::SEARCH_INDEX_STORE;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatefulComponent {
    S1FtStructured,
    S2Vector,
    S3DedupLedger,
    S4ReindexCursor,
    S5FilterCache,
}

impl StatefulComponent {
    pub fn register() -> [StatefulComponent; 5] {
        [
            StatefulComponent::S1FtStructured,
            StatefulComponent::S2Vector,
            StatefulComponent::S3DedupLedger,
            StatefulComponent::S4ReindexCursor,
            StatefulComponent::S5FilterCache,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            StatefulComponent::S1FtStructured => "S1",
            StatefulComponent::S2Vector => "S2",
            StatefulComponent::S3DedupLedger => "S3",
            StatefulComponent::S4ReindexCursor => "S4",
            StatefulComponent::S5FilterCache => "S5",
        }
    }

    pub fn is_derived_rebuildable(self) -> bool {
        match self {
            StatefulComponent::S1FtStructured
            | StatefulComponent::S2Vector
            | StatefulComponent::S3DedupLedger
            | StatefulComponent::S4ReindexCursor
            | StatefulComponent::S5FilterCache => true,
        }
    }

    pub fn filled_by(self) -> &'static str {
        match self {
            StatefulComponent::S1FtStructured => {
                "SRCH-P04 (IndexBackend + FT/structured) + SRCH-P06 (indexer)"
            }
            StatefulComponent::S2Vector => "SRCH-P05 (vector shape) + SRCH-P06 (embedder/indexer)",
            StatefulComponent::S3DedupLedger => "SRCH-P06 (the indexer's idempotency ledger)",
            StatefulComponent::S4ReindexCursor => "SRCH-P16 (reindex-from-source cursor)",
            StatefulComponent::S5FilterCache => "SRCH-P13 (the list_objects filter/result cache)",
        }
    }
}

pub fn derived_state_invariant_holds() -> bool {
    StatefulComponent::register()
        .iter()
        .all(|c| c.is_derived_rebuildable())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    Unrecoverable(KmsError),
    SegmentUnreadable,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::Unrecoverable(e) => write!(
                f,
                "the per-tenant index DEK is unavailable (crypto-shredded or wrong region): {e} - \
                 the per-tenant index directory is UNRECOVERABLE (never a plaintext fall-through)"
            ),
            LayoutError::SegmentUnreadable => {
                write!(
                    f,
                    "an index segment ciphertext failed to open under the per-tenant index DEK"
                )
            }
        }
    }
}

impl std::error::Error for LayoutError {}

#[derive(Clone, Debug)]
pub struct PerTenantIndexLayout {
    pub store: &'static str,
    pub tenant: TenantId,
    pub region: Region,
    pub residency: ResidencyTag,
    pub index_dek_ref: PiiKeyRef,
}

impl PerTenantIndexLayout {
    pub fn create(
        pin: &SearchDekPin,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<PerTenantIndexLayout, KmsError> {
        let index_dek_ref = pin.reserve(tenant, region)?;
        Ok(PerTenantIndexLayout {
            store: SEARCH_INDEX_STORE,
            tenant: tenant.clone(),
            region: region.clone(),
            residency: ResidencyTag::pinned_to(region.clone()),
            index_dek_ref,
        })
    }

    pub fn seal(
        &self,
        pin: &SearchDekPin,
        plaintext: &[u8],
    ) -> Result<([u8; NONCE_LEN], Vec<u8>), LayoutError> {
        let dek = pin
            .resolve(&self.index_dek_ref, &self.region)
            .map_err(LayoutError::Unrecoverable)?;
        Ok(dek.seal(plaintext))
    }

    pub fn open(
        &self,
        pin: &SearchDekPin,
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, LayoutError> {
        let dek = pin
            .resolve(&self.index_dek_ref, &self.region)
            .map_err(LayoutError::Unrecoverable)?;
        dek.open(nonce, ciphertext)
            .ok_or(LayoutError::SegmentUnreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use myelin_storage::{KeyClass, KmsEngine};

    fn pin() -> SearchDekPin {
        SearchDekPin::new(Arc::new(KmsEngine::new()))
    }
    fn t() -> TenantId {
        TenantId::from_token("acme")
    }
    fn r() -> Region {
        Region("fr-par".into())
    }

    #[test]
    fn layout_is_created_encrypted_from_birth_and_residency_pinned() {
        let pin = pin();
        let layout = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("create the directory");

        assert_eq!(
            layout.store, SEARCH_INDEX_STORE,
            "the per-tenant search index store"
        );
        assert_eq!(
            layout.tenant,
            t(),
            "the directory is keyed to its tenant (first partition key)"
        );
        assert_eq!(
            layout.region,
            r(),
            "the directory lives in the tenant's cell region"
        );
        assert_eq!(
            layout.residency,
            ResidencyTag::pinned_to(r()),
            "residency-pinned to its region"
        );
        assert_eq!(
            layout.index_dek_ref.class,
            KeyClass::Tenant,
            "sealed under the per-tenant index DEK"
        );
        assert_eq!(
            layout.index_dek_ref.to_uri(),
            "kms://acme/0/tenant",
            "the encrypted-from-birth ref"
        );
    }

    #[test]
    fn a_segment_is_sealed_under_the_index_dek_and_round_trips() {
        let pin = pin();
        let layout = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("create");

        let body = b"a future FT+vector index segment's body";
        let (nonce, ct) = layout
            .seal(&pin, body)
            .expect("seal under the per-tenant index DEK");
        assert_ne!(
            &ct[..],
            &body[..],
            "the segment is ciphertext at rest (encrypted-from-birth)"
        );
        let plain = layout
            .open(&pin, &nonce, &ct)
            .expect("open the sealed segment");
        assert_eq!(
            plain, body,
            "the sealed segment round-trips under the per-tenant index DEK"
        );
    }

    #[test]
    fn destroying_the_dek_renders_the_directory_unrecoverable() {
        let pin = pin();
        let layout = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("create");

        let (nonce, ct) = layout.seal(&pin, b"sensitive analyzed text").expect("seal");
        assert!(
            layout.open(&pin, &nonce, &ct).is_ok(),
            "readable before the shred"
        );

        assert!(
            pin.destroy_tenant_index_dek(&t(), &r()).unwrap(),
            "the per-tenant index DEK is destroyable (the tenant-decommission shred lever fires)"
        );

        match layout.open(&pin, &nonce, &ct) {
            Err(LayoutError::Unrecoverable(_)) => {}
            other => {
                panic!("a crypto-shredded index directory must be UNRECOVERABLE, got {other:?}")
            }
        }
        assert!(matches!(
            layout.seal(&pin, b"x"),
            Err(LayoutError::Unrecoverable(_))
        ));
    }

    #[test]
    fn re_creating_the_directory_does_not_rotate_the_dek() {
        let pin = pin();
        let a = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("first create");
        let b = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("re-create on restart");
        assert_eq!(
            a.index_dek_ref, b.index_dek_ref,
            "the same per-tenant index DEK (no silent rotation)"
        );

        let (nonce, ct) = a.seal(&pin, b"pre-restart segment").expect("seal");
        assert_eq!(
            b.open(&pin, &nonce, &ct).expect("open after re-create"),
            b"pre-restart segment"
        );
    }

    #[test]
    fn s1_s5_register_is_complete_derived_and_each_names_its_filler() {
        let reg = StatefulComponent::register();
        assert_eq!(reg.len(), 5, "the register is exactly S1–S5");
        let ids: Vec<&str> = reg.iter().map(|c| c.id()).collect();
        assert_eq!(ids, ["S1", "S2", "S3", "S4", "S5"], "S1–S5 in order");

        assert!(
            derived_state_invariant_holds(),
            "no Search component is a system of record"
        );
        for c in reg {
            assert!(
                c.is_derived_rebuildable(),
                "{} is derived/rebuildable",
                c.id()
            );
            assert!(
                !c.filled_by().is_empty(),
                "{} names the slice that fills it",
                c.id()
            );
        }
    }
}
