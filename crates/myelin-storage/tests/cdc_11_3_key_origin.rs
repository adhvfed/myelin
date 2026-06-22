//! Contract 11.3 CDC pair — the `KeyOrigin` trait half (P-ST-07 / global P-094).
//!
//! Row 11.3 spans BOTH the KMS hierarchy (P-ST-06 / P-058, covered by `cdc_11_3_kms.rs`) AND the
//! `KeyOrigin` trait (THIS prompt). This CDC pair covers the KeyOrigin half:
//!
//! - the **PROVIDER** is `myelin-storage` — the [`KeyOrigin`] trait + its three origins + the
//!   [`IndexAdmission`] seam this prompt ships;
//! - the **CONSUMER** is an INDEX BUILDER (modelled here as a tiny `IndexBuilder` — the call shape
//!   Search and the Agent Fabric use). Before building a plaintext-derived index over a class, it
//!   consults `can_derive_plaintext_index()` (via [`IndexAdmission::for_origin`]). A HYOK class is
//!   REFUSED a plaintext index BY CONSTRUCTION — *you cannot index what you cannot decrypt*.
//!
//! If `can_derive_plaintext_index` / the `KeyOrigin` shape / the `IndexAdmission` verdict drift,
//! this stops compiling/passing — exactly the consumer-driven contract Search/Agent depend on
//! (the full skip drill D-S10 lands with those subsystems; this pins the seam they call).

use myelin_storage::{
    Byok, Dek, DekHandle, Hyok, HyokKeyService, HyokServiceDenied, IndexAdmission, KekId,
    KeyOrigin, KmsEngine, PlatformManaged, WrappedDek, KEY_LEN,
};
use myelin_tenancy::{Region, TenantId};

// A deterministic in-process customer HYOK key service — the out-of-platform key holder. Its
// plaintext key NEVER enters the consumer; the consumer only ever calls `can_derive...`.
struct CustomerKeyService;
impl HyokKeyService for CustomerKeyService {
    fn wrap(&self, dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
        // Stand-in wrap (the real KMIP adapter is the [OPEN → P6/LEGAL] follow-on).
        let _ = dek;
        Ok(WrappedDek {
            nonce: [0u8; 12],
            wrapped: vec![0u8; KEY_LEN],
            kek_epoch: 0,
        })
    }
    fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
        Ok(DekHandle::from_raw([3u8; KEY_LEN]))
    }
    fn destroy(&self) {}
}

/// The CONSUMER of 11.3's KeyOrigin half: an index builder. It indexes a class IFF its key origin
/// can derive a plaintext index. This is exactly the Search/Agent call shape (D-S10).
struct IndexBuilder {
    indexed: Vec<String>,
    skipped_hyok: Vec<String>,
}
impl IndexBuilder {
    fn new() -> Self {
        IndexBuilder {
            indexed: Vec::new(),
            skipped_hyok: Vec::new(),
        }
    }
    /// Try to build a plaintext-derived index over `class_name`, governed by its `origin`. A HYOK
    /// origin is skipped BY CONSTRUCTION (the consumer cannot bypass it — it has no decrypt path).
    fn build_index(&mut self, class_name: &str, origin: &dyn KeyOrigin) {
        match IndexAdmission::for_origin(origin) {
            IndexAdmission::Admit => self.indexed.push(class_name.to_string()),
            IndexAdmission::SkipHyok => self.skipped_hyok.push(class_name.to_string()),
        }
    }
}

#[test]
fn cdc_11_3_index_builder_consults_can_derive_plaintext_index() {
    let engine = KmsEngine::new();
    engine.ensure_kek(&KekId::new(
        TenantId("acme".into()),
        Region("eu-west".into()),
    ));

    let platform = PlatformManaged::new(&engine, Region("eu-west".into()));
    let byok = Byok::new(&engine, Region("eu-west".into()), "kms-customer://acme/k1");
    let hyok = Hyok::new(CustomerKeyService);

    let mut builder = IndexBuilder::new();
    // Three classes, each under a different origin — the consumer asks the provider before indexing.
    builder.build_index("issue_fields_platform", &platform);
    builder.build_index("profile_bio_byok", &byok);
    builder.build_index("repo_contents_hyok", &hyok);

    // The contract: platform + BYOK classes ARE indexed; the HYOK class is structurally skipped.
    assert_eq!(
        builder.indexed,
        vec!["issue_fields_platform", "profile_bio_byok"]
    );
    assert_eq!(builder.skipped_hyok, vec!["repo_contents_hyok"]);

    // The HYOK class NEVER ended up in the indexed set — cross-check the no-leak property.
    assert!(
        !builder.indexed.iter().any(|c| c.contains("hyok")),
        "11.3: a HYOK class can NEVER have a plaintext index built (you cannot index what you \
         cannot decrypt — enforced by code)"
    );
}

#[test]
fn cdc_11_3_byok_wraps_under_customer_path_full_capability() {
    // The provider property the consumer (a BYOK tenant's search) depends on: BYOK is
    // full-capability while the key is live, wrapping under the customer key path.
    let engine = KmsEngine::new();
    engine.ensure_kek(&KekId::new(
        TenantId("acme".into()),
        Region("eu-west".into()),
    ));
    let byok = Byok::new(
        &engine,
        Region("eu-west".into()),
        "kms-customer://acme/master",
    );

    assert_eq!(byok.customer_key_path(), "kms-customer://acme/master");
    assert!(
        byok.can_derive_plaintext_index(),
        "BYOK: full capability while the key is live"
    );

    let dek = Dek::generate();
    let wrapped = byok
        .wrap(&dek, TenantId("acme".into()))
        .expect("byok wrap under customer path");
    let _handle = byok
        .unwrap(&wrapped, TenantId("acme".into()))
        .expect("byok unwrap (key live)");
}
