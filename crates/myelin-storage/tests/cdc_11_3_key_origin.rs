use myelin_storage::{
    Byok, Dek, DekHandle, Hyok, HyokKeyService, HyokServiceDenied, IndexAdmission, KekId,
    KeyOrigin, KmsEngine, PlatformManaged, WrappedDek, KEY_LEN,
};
use myelin_tenancy::{Region, TenantId};

struct CustomerKeyService;
impl HyokKeyService for CustomerKeyService {
    fn wrap(&self, dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
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
    builder.build_index("issue_fields_platform", &platform);
    builder.build_index("profile_bio_byok", &byok);
    builder.build_index("repo_contents_hyok", &hyok);

    assert_eq!(
        builder.indexed,
        vec!["issue_fields_platform", "profile_bio_byok"]
    );
    assert_eq!(builder.skipped_hyok, vec!["repo_contents_hyok"]);

    assert!(
        !builder.indexed.iter().any(|c| c.contains("hyok")),
        "11.3: a HYOK class can NEVER have a plaintext index built (you cannot index what you \
         cannot decrypt - enforced by code)"
    );
}

#[test]
fn cdc_11_3_byok_wraps_under_customer_path_full_capability() {
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
