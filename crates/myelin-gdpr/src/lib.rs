extern crate self as myelin_gdpr;

use myelin_identity::Principal;
use myelin_tenancy::TenantId as TenancyTenantId;
use serde::{Deserialize, Serialize};

pub use myelin_gdpr_macros::PersonalData;

pub mod __registry;
pub use __registry::{
    default_data_role_default, DataRoleDefault, ErasureKeyClass, HasPersonalData,
    PersonalDataField, PersonalDataTags, SpecialCategoryFlag,
};

pub mod dpia;
pub use dpia::{dpia_markers, dpia_markers_of, DpiaMarker, DpiaRouter, DpiaVerdict};

pub type TenantId = TenancyTenantId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectRef {
    pub principal: Principal,
}

impl SubjectRef {
    pub fn new(principal: Principal) -> SubjectRef {
        SubjectRef { principal }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    TenantContent,
    PlatformOperational,
}

impl DataRole {
    pub fn to_envelope(self) -> myelin_events::DataRole {
        match self {
            DataRole::TenantContent => myelin_events::DataRole::Processor,
            DataRole::PlatformOperational => myelin_events::DataRole::Controller,
        }
    }

    pub fn from_envelope(role: myelin_events::DataRole) -> DataRole {
        match role {
            myelin_events::DataRole::Processor => DataRole::TenantContent,
            myelin_events::DataRole::Controller => DataRole::PlatformOperational,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataCategory {
    ContactInfo,
    Identifier,
    Content,
    Behavioural,
    SpecialCategory(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LawfulBasis {
    Contract,
    LegitimateInterest(String),
    Consent(String),
    LegalObligation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetentionClass {
    TenantPolicy,
    Fixed(core::time::Duration),
    UntilContractEnd,
    AuditCarveOut(core::time::Duration),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErasureMethod {
    Pseudonymise,
    CryptoShred(String),
    PurgeReindex,
    CarveOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EraseScope {
    Subject {
        subject: SubjectRef,
        tenant: TenantId,
    },
    Tenant(TenantId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub operation: String,
    pub content_hash: String,
    #[serde(default)]
    pub key_epoch_destroyed: Option<u64>,
}

impl Receipt {
    pub fn content_addressed(
        operation: &str,
        holder: &str,
        subject: &str,
        tenant: &str,
        outcome: &str,
        key_epoch_destroyed: Option<u64>,
        at_ms: u64,
    ) -> Receipt {
        let body = format!(
            "op={operation}\u{1f}holder={holder}\u{1f}subject={subject}\u{1f}tenant={tenant}\
             \u{1f}outcome={outcome}\u{1f}key_epoch={}\u{1f}at={at_ms}",
            match key_epoch_destroyed {
                Some(e) => e.to_string(),
                None => "none".to_string(),
            }
        );
        let digest = blake3::hash(body.as_bytes());
        Receipt {
            operation: operation.to_string(),
            content_hash: format!("blake3:{}", hex::encode(digest.as_bytes())),
            key_epoch_destroyed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocateReport {
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableBundle {
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RectifyReceipt {
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestrictReceipt {
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseReceipt {
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DsrError(pub String);

pub type Result<T> = core::result::Result<T, DsrError>;

pub trait PersonalDataHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> Result<LocateReport>;
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> Result<PortableBundle>;
    fn rectify(&self, subject: &SubjectRef, patch: Patch) -> Result<RectifyReceipt>;
    fn restrict(&self, subject: &SubjectRef, on: bool) -> Result<RestrictReceipt>;
    fn erase(&self, scope: EraseScope) -> Result<EraseReceipt>;
}

pub mod classify_fixture {
    use super::{DataCategory, DataRole, ErasureMethod, LawfulBasis, PersonalData, RetentionClass};

    #[derive(PersonalData)]
    pub struct ContactRecord {
        pub id: u64,
        #[personal_data(category = ContactInfo, role = TenantContent, basis = Contract, retention = TenantPolicy, erasure = CryptoShred(subject_dek), subject_locator = "id")]
        pub email: String,
    }

    impl ContactRecord {
        pub fn new(id: u64, email: String) -> ContactRecord {
            let _category = DataCategory::ContactInfo;
            let _role = DataRole::TenantContent;
            let _basis = LawfulBasis::Contract;
            let _retention = RetentionClass::TenantPolicy;
            let _erasure = ErasureMethod::CryptoShred("subject_dek".into());
            ContactRecord { id, email }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        )
    }

    fn subject() -> SubjectRef {
        SubjectRef::new(principal())
    }

    #[test]
    fn personal_data_holder_shape_is_frozen_and_object_safe() {
        struct Store;
        impl PersonalDataHolder for Store {
            fn locate(&self, _s: &SubjectRef, _t: TenantId) -> Result<LocateReport> {
                Err(DsrError("locate body → GDPR M1 (P-GA-05)".into()))
            }
            fn export(&self, _s: &SubjectRef, _t: TenantId) -> Result<PortableBundle> {
                Err(DsrError("export body → GDPR M1 (P-GA-05; 10.4)".into()))
            }
            fn rectify(&self, _s: &SubjectRef, _p: Patch) -> Result<RectifyReceipt> {
                Err(DsrError("rectify body → GDPR M1 (P-GA-05)".into()))
            }
            fn restrict(&self, _s: &SubjectRef, _on: bool) -> Result<RestrictReceipt> {
                Err(DsrError("restrict body → GDPR M1 (P-GA-05)".into()))
            }
            fn erase(&self, _scope: EraseScope) -> Result<EraseReceipt> {
                Err(DsrError(
                    "erase = crypto-shred → GDPR M1 (P-GA-05; ADR-12.3)".into(),
                ))
            }
        }
        let holder: Box<dyn PersonalDataHolder> = Box::new(Store);
        let subj = subject();
        assert!(holder.locate(&subj, TenantId::from_token("acme")).is_err());
        assert!(holder
            .erase(EraseScope::Tenant(TenantId::from_token("acme")))
            .is_err());
    }

    #[test]
    fn data_role_serializes_to_the_frozen_2_1_envelope_field() {
        assert_eq!(
            DataRole::TenantContent.to_envelope(),
            myelin_events::DataRole::Processor
        );
        assert_eq!(
            DataRole::PlatformOperational.to_envelope(),
            myelin_events::DataRole::Controller
        );
        for role in [DataRole::TenantContent, DataRole::PlatformOperational] {
            assert_eq!(DataRole::from_envelope(role.to_envelope()), role);
        }
        for env in [
            myelin_events::DataRole::Processor,
            myelin_events::DataRole::Controller,
        ] {
            assert_eq!(DataRole::from_envelope(env).to_envelope(), env);
        }
        let json = serde_json::to_string(&DataRole::TenantContent).unwrap();
        assert_eq!(json, "\"TenantContent\"");
        assert_eq!(
            serde_json::from_str::<DataRole>(&json).unwrap(),
            DataRole::TenantContent
        );
    }

    #[test]
    fn core_types_round_trip_serialize() {
        let s = subject();
        let back: SubjectRef = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);

        let t = TenantId::from_token("acme");
        let t_back: TenantId = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(t_back, t);

        for scope in [
            EraseScope::Subject {
                subject: s.clone(),
                tenant: t.clone(),
            },
            EraseScope::Tenant(t.clone()),
        ] {
            let sc_back: EraseScope =
                serde_json::from_str(&serde_json::to_string(&scope).unwrap()).unwrap();
            assert_eq!(sc_back, scope);
        }

        let r = Receipt {
            operation: "erase".into(),
            content_hash: "blake3:deadbeef".into(),
            key_epoch_destroyed: Some(3),
        };
        let r_back: Receipt = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r_back, r);

        let legacy: Receipt =
            serde_json::from_str(r#"{"operation":"locate","content_hash":"blake3:00"}"#).unwrap();
        assert_eq!(legacy.key_epoch_destroyed, None);
    }

    #[test]
    fn content_addressed_receipt_is_deterministic_and_records_the_key_epoch() {
        let r1 = Receipt::content_addressed(
            "erase",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "crypto_shred",
            Some(7),
            1_000,
        );
        let r2 = Receipt::content_addressed(
            "erase",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "crypto_shred",
            Some(7),
            1_000,
        );
        assert_eq!(r1, r2);
        assert!(r1.content_hash.starts_with("blake3:"));
        assert_eq!(
            r1.key_epoch_destroyed,
            Some(7),
            "the destroyed key epoch is recorded"
        );
        let r3 = Receipt::content_addressed(
            "erase",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "crypto_shred",
            Some(8),
            1_000,
        );
        assert_ne!(r1.content_hash, r3.content_hash);
        let loc = Receipt::content_addressed(
            "locate",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "located",
            None,
            5,
        );
        assert_eq!(loc.key_epoch_destroyed, None);
        assert_ne!(loc.content_hash, r1.content_hash);
    }

    #[test]
    fn personal_data_attribute_parses_the_five_tag_keys() {
        #[derive(PersonalData)]
        struct Tagged {
            #[personal_data(
                category = Identifier,
                role = PlatformOperational,
                basis = LegitimateInterest(ops_lia),
                retention = Fixed(90d),
                erasure = PurgeReindex,
                subject_locator = "principal_id"
            )]
            principal_id: String,
        }
        let t = Tagged {
            principal_id: "p-1".into(),
        };
        assert_eq!(t.principal_id, "p-1");

        let rec = classify_fixture::ContactRecord::new(7, "a@b.test".into());
        assert_eq!(rec.id, 7);
        assert_eq!(rec.email, "a@b.test");
    }

    #[test]
    fn five_tag_enum_names_and_variants_exist_and_round_trip() {
        let categories = [
            DataCategory::ContactInfo,
            DataCategory::Identifier,
            DataCategory::Content,
            DataCategory::Behavioural,
            DataCategory::SpecialCategory("health".into()),
        ];
        let roles = [DataRole::TenantContent, DataRole::PlatformOperational];
        let bases = [
            LawfulBasis::Contract,
            LawfulBasis::LegitimateInterest("lia-1".into()),
            LawfulBasis::Consent("consent-1".into()),
            LawfulBasis::LegalObligation,
        ];
        let retentions = [
            RetentionClass::TenantPolicy,
            RetentionClass::Fixed(core::time::Duration::from_secs(86_400)),
            RetentionClass::UntilContractEnd,
            RetentionClass::AuditCarveOut(core::time::Duration::from_secs(86_400 * 365)),
        ];
        let erasures = [
            ErasureMethod::Pseudonymise,
            ErasureMethod::CryptoShred("subject_dek".into()),
            ErasureMethod::PurgeReindex,
            ErasureMethod::CarveOut,
        ];

        fn round_trip<T>(values: &[T])
        where
            T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
        {
            for v in values {
                let json = serde_json::to_string(v).unwrap();
                let back: T = serde_json::from_str(&json).unwrap();
                assert_eq!(&back, v, "tag enum variant must round-trip: {json}");
            }
        }
        round_trip(&categories);
        round_trip(&roles);
        round_trip(&bases);
        round_trip(&retentions);
        round_trip(&erasures);

        assert_eq!(
            serde_json::to_string(&DataCategory::ContactInfo).unwrap(),
            "\"ContactInfo\""
        );
        assert_eq!(
            serde_json::to_string(&ErasureMethod::PurgeReindex).unwrap(),
            "\"PurgeReindex\""
        );
    }

    #[test]
    fn derive_emits_a_complete_registry_entry_for_every_tag_form() {
        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct EveryTag {
            #[personal_data(
                category = ContactInfo,
                role = TenantContent,
                basis = Contract,
                retention = TenantPolicy,
                erasure = Pseudonymise,
                subject_locator = "principal_id"
            )]
            contact: String,
            #[personal_data(
                category = SpecialCategory(health),
                role = PlatformOperational,
                basis = Consent(c-1),
                retention = Fixed(90d),
                erasure = CryptoShred(subject_dek),
                subject_locator = "subject_ref"
            )]
            sensitive: String,
            #[personal_data(
                category = Identifier,
                role = TenantContent,
                basis = LegitimateInterest(ops_lia),
                retention = UntilContractEnd,
                erasure = PurgeReindex,
                subject_locator = "id"
            )]
            handle: String,
            row_version: u64,
        }

        let fields = EveryTag::personal_data_fields();
        assert_eq!(
            fields.len(),
            3,
            "one entry per TAGGED field, the non-PII field has none"
        );

        assert!(fields.iter().all(|f| f.owning_struct == "EveryTag"));
        let by_field: std::collections::HashMap<&str, &PersonalDataField> =
            fields.iter().map(|f| (f.field, f)).collect();

        let contact = by_field["contact"];
        assert_eq!(contact.tags.category, "ContactInfo");
        assert_eq!(contact.tags.role, "TenantContent");
        assert_eq!(contact.tags.basis, "Contract");
        assert_eq!(contact.tags.retention, "TenantPolicy");
        assert_eq!(contact.tags.erasure, "Pseudonymise");
        assert_eq!(contact.tags.subject_locator, "principal_id");

        let sensitive = by_field["sensitive"];
        assert_eq!(sensitive.tags.category, "SpecialCategory(health)");
        assert_eq!(sensitive.tags.basis, "Consent(c-1)");
        assert_eq!(sensitive.tags.retention, "Fixed(90d)");
        assert_eq!(sensitive.tags.erasure, "CryptoShred(subject_dek)");
        assert_eq!(
            sensitive.erasure_key_class(),
            Some(ErasureKeyClass::SubjectDek)
        );
        assert_eq!(
            sensitive.is_special_category(),
            Some(SpecialCategoryFlag { kind: "health" })
        );

        let handle = by_field["handle"];
        assert_eq!(handle.tags.basis, "LegitimateInterest(ops_lia)");
        assert_eq!(handle.tags.erasure, "PurgeReindex");
        assert_eq!(handle.erasure_key_class(), None);
        assert_eq!(handle.is_special_category(), None);
    }

    #[test]
    fn subject_locator_accessor_is_structural() {
        use classify_fixture::ContactRecord;
        assert_eq!(ContactRecord::subject_locator("email"), Some("id"));
        assert_eq!(ContactRecord::subject_locator("id"), None);
        assert_eq!(ContactRecord::subject_locator("does_not_exist"), None);
        let fields = ContactRecord::personal_data_fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field, "email");
        assert_eq!(fields[0].tags.erasure, "CryptoShred(subject_dek)");
    }

    #[test]
    fn derive_is_uniform_empty_registry_for_a_pii_free_struct() {
        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct NoPii {
            id: u64,
            region: String,
        }
        assert!(NoPii::personal_data_fields().is_empty());
        assert_eq!(NoPii::subject_locator("id"), None);

        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct TupleRow(u64, String);
        assert!(TupleRow::personal_data_fields().is_empty());
        assert_eq!(TupleRow::subject_locator("0"), None);

        #[derive(PersonalData)]
        struct UnitRow;
        assert!(UnitRow::personal_data_fields().is_empty());
    }
}

pub mod untagged_pii_rejection_doc {}
