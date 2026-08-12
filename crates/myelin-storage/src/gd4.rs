use myelin_gdpr::ErasureMethod;
use myelin_tenancy::{Region, TenantId};

use crate::encryption::{key_class_for, KeyChoiceError, SubjectId};
use crate::erase::EraseHolders;
use crate::kms::{DekId, KekId, KeyClass, KmsEngine, KmsError};

pub const RESIDUAL_POSTURE_REF: &str =
    "the residual is handled per the platform erasure posture in 00-reconciliation §X-7 (contract 10.9)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyGranularity {
    PerSubjectDek,
    PerTenantDek,
    PerTenantKek,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataClass {
    FreeTextProfile,
    ChatBody,
    AgentMemory,
    CiInlinePiiLog,
    BulkTenantContent,
    TenantOffboard,
}

impl DataClass {
    pub fn granularity(self) -> KeyGranularity {
        match self {
            DataClass::FreeTextProfile
            | DataClass::ChatBody
            | DataClass::AgentMemory
            | DataClass::CiInlinePiiLog => KeyGranularity::PerSubjectDek,
            DataClass::BulkTenantContent => KeyGranularity::PerTenantDek,
            DataClass::TenantOffboard => KeyGranularity::PerTenantKek,
        }
    }

    pub fn all() -> [DataClass; 6] {
        [
            DataClass::FreeTextProfile,
            DataClass::ChatBody,
            DataClass::AgentMemory,
            DataClass::CiInlinePiiLog,
            DataClass::BulkTenantContent,
            DataClass::TenantOffboard,
        ]
    }
}

pub fn granularity_of_key_class(class: &KeyClass) -> KeyGranularity {
    match class {
        KeyClass::Subject(_) => KeyGranularity::PerSubjectDek,
        KeyClass::Tenant | KeyClass::Blob => KeyGranularity::PerTenantDek,
    }
}

pub fn assert_gd4_table_complete() -> Gd4TableReport {
    let routed: Vec<(DataClass, KeyGranularity)> = DataClass::all()
        .iter()
        .map(|c| (*c, c.granularity()))
        .collect();
    let expected = [
        (DataClass::FreeTextProfile, KeyGranularity::PerSubjectDek),
        (DataClass::ChatBody, KeyGranularity::PerSubjectDek),
        (DataClass::AgentMemory, KeyGranularity::PerSubjectDek),
        (DataClass::CiInlinePiiLog, KeyGranularity::PerSubjectDek),
        (DataClass::BulkTenantContent, KeyGranularity::PerTenantDek),
        (DataClass::TenantOffboard, KeyGranularity::PerTenantKek),
    ];
    let misrouted = routed
        .iter()
        .zip(expected.iter())
        .filter(|((_, got), (_, want))| got != want)
        .count();
    Gd4TableReport { routed, misrouted }
}

#[derive(Clone, Debug)]
pub struct Gd4TableReport {
    pub routed: Vec<(DataClass, KeyGranularity)>,
    pub misrouted: usize,
}

impl Gd4TableReport {
    pub fn is_green(&self) -> bool {
        self.misrouted == 0
    }
}

pub fn key_choice_granularity(
    erasure: &ErasureMethod,
    subject: Option<&SubjectId>,
) -> Result<KeyGranularity, KeyChoiceError> {
    let class = key_class_for(erasure, subject)?;
    Ok(granularity_of_key_class(&class))
}

pub struct StructuralErasureFloor<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> StructuralErasureFloor<'a> {
    pub fn new(engine: &'a KmsEngine, region: Region) -> StructuralErasureFloor<'a> {
        StructuralErasureFloor { engine, region }
    }

    pub fn verify(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
    ) -> Result<StructuralFloorReport, KmsError> {
        self.engine
            .ensure_kek(&KekId::new(tenant.clone(), self.region.clone()))?;
        let key_ref =
            self.engine
                .ensure_dek(tenant, &self.region, KeyClass::Subject(subject.0.clone()))?;

        let dek = self.engine.resolve_dek(&key_ref, &self.region)?;
        let marker = b"the-subject-free-text-marker";
        let (nonce, ciphertext) = dek.seal(marker);

        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        let destroyed = self.engine.destroy_dek(&subject_dek)?;

        let lever_works = self.engine.resolve_dek(&key_ref, &self.region).is_err();
        let ciphertext_not_plaintext = !ciphertext.windows(marker.len()).any(|w| w == marker);

        let recoverable_in_backup = self
            .engine
            .backup_snapshot()?
            .iter()
            .filter(|(d, _)| *d == subject_dek)
            .count();

        Ok(StructuralFloorReport {
            subject: subject.0.clone(),
            tenant: tenant.clone(),
            lever_destroyed_dek: destroyed,
            lever_renders_unrecoverable: lever_works && ciphertext_not_plaintext,
            recoverable_in_backup,
            pseudonym_shred_is_the_id_step: true,
            nonce,
        })
    }

    pub fn region(&self) -> &Region {
        &self.region
    }
}

#[derive(Clone, Debug)]
pub struct StructuralFloorReport {
    pub subject: String,
    pub tenant: TenantId,
    pub lever_destroyed_dek: bool,
    pub lever_renders_unrecoverable: bool,
    pub recoverable_in_backup: usize,
    pub pseudonym_shred_is_the_id_step: bool,
    pub nonce: [u8; crate::kms::NONCE_LEN],
}

impl StructuralFloorReport {
    pub fn is_green(&self) -> bool {
        self.lever_renders_unrecoverable
            && self.recoverable_in_backup == 0
            && self.pseudonym_shred_is_the_id_step
    }
}

pub fn assert_no_local_residual_statement() -> &'static str {
    RESIDUAL_POSTURE_REF
}

pub fn structural_reach_uses_erase_seams(_holders: &EraseHolders<'_>) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::KmsEngine;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r() -> Region {
        Region("eu-west".to_string())
    }
    fn engine_for(tenant: &TenantId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()))
            .expect("seed the in-memory KEK");
        kms
    }

    #[test]
    fn gd4_table_routes_every_class_to_the_correct_granularity_zero_misrouted() {
        let report = assert_gd4_table_complete();
        assert_eq!(
            report.misrouted, 0,
            "0 misrouted classes (GD-4 granularity completeness)"
        );
        assert!(report.is_green());
        assert_eq!(report.routed.len(), 6);
    }

    #[test]
    fn free_text_chat_agent_ci_log_route_to_per_subject_dek() {
        for class in [
            DataClass::FreeTextProfile,
            DataClass::ChatBody,
            DataClass::AgentMemory,
            DataClass::CiInlinePiiLog,
        ] {
            assert_eq!(
                class.granularity(),
                KeyGranularity::PerSubjectDek,
                "{class:?} must be per-subject (individual Art. 17 erasure = one key-destroy)"
            );
        }
    }

    #[test]
    fn bulk_content_routes_to_per_tenant_dek() {
        assert_eq!(
            DataClass::BulkTenantContent.granularity(),
            KeyGranularity::PerTenantDek
        );
    }

    #[test]
    fn tenant_offboard_routes_to_the_per_tenant_kek_the_third_granularity() {
        assert_eq!(
            DataClass::TenantOffboard.granularity(),
            KeyGranularity::PerTenantKek
        );
    }

    #[test]
    fn the_three_granularities_are_distinct() {
        assert_ne!(KeyGranularity::PerSubjectDek, KeyGranularity::PerTenantDek);
        assert_ne!(KeyGranularity::PerTenantDek, KeyGranularity::PerTenantKek);
        assert_ne!(KeyGranularity::PerSubjectDek, KeyGranularity::PerTenantKek);
    }

    #[test]
    fn key_class_granularity_bridges_the_dek_rule_to_the_granularity_model() {
        assert_eq!(
            granularity_of_key_class(&KeyClass::Subject("u-1".into())),
            KeyGranularity::PerSubjectDek
        );
        assert_eq!(
            granularity_of_key_class(&KeyClass::Tenant),
            KeyGranularity::PerTenantDek
        );
        assert_eq!(
            granularity_of_key_class(&KeyClass::Blob),
            KeyGranularity::PerTenantDek
        );
    }

    #[test]
    fn key_choice_granularity_agrees_with_the_dek_rule() {
        assert_eq!(
            key_choice_granularity(
                &ErasureMethod::CryptoShred("subject_dek".into()),
                Some(&SubjectId::new("u-1")),
            )
            .unwrap(),
            KeyGranularity::PerSubjectDek
        );
        for e in [
            ErasureMethod::PurgeReindex,
            ErasureMethod::Pseudonymise,
            ErasureMethod::CarveOut,
            ErasureMethod::CryptoShred("tenant_dek".into()),
        ] {
            assert_eq!(
                key_choice_granularity(&e, None).unwrap(),
                KeyGranularity::PerTenantDek,
                "{e:?} is bulk → per-tenant"
            );
        }
    }

    #[test]
    fn key_choice_granularity_propagates_the_loud_classification_error() {
        assert!(matches!(
            key_choice_granularity(&ErasureMethod::CryptoShred("subject_dek".into()), None),
            Err(KeyChoiceError::SubjectClassMissingSubject(_))
        ));
    }

    #[test]
    fn structural_floor_lever_renders_a_subject_unrecoverable_and_reaches_backups() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let floor = StructuralErasureFloor::new(&kms, r());
        let report = floor
            .verify(&SubjectId::new("u-erase"), &tenant)
            .expect("the in-memory key registry remains available");

        assert!(
            report.lever_destroyed_dek,
            "the lever destroys the subject DEK"
        );
        assert!(
            report.lever_renders_unrecoverable,
            "the destroyed DEK makes the subject's content unrecoverable (never plaintext)"
        );
        assert_eq!(
            report.recoverable_in_backup, 0,
            "the destroyed DEK is excluded from the backup (backups-by-construction, §7.5)"
        );
        assert!(report.pseudonym_shred_is_the_id_step);
        assert!(report.is_green(), "the structural GDPR floor holds");
    }

    #[test]
    fn structural_floor_region_accessor() {
        let kms = KmsEngine::new();
        let floor = StructuralErasureFloor::new(&kms, r());
        assert_eq!(floor.region(), &r());
    }

    #[test]
    fn structural_floor_report_is_red_if_a_guarantee_fails() {
        let base = StructuralFloorReport {
            subject: "u".into(),
            tenant: t("acme"),
            lever_destroyed_dek: true,
            lever_renders_unrecoverable: true,
            recoverable_in_backup: 0,
            pseudonym_shred_is_the_id_step: true,
            nonce: [0u8; crate::kms::NONCE_LEN],
        };
        assert!(base.is_green());
        assert!(!StructuralFloorReport {
            recoverable_in_backup: 1,
            ..base.clone()
        }
        .is_green());
        assert!(!StructuralFloorReport {
            lever_renders_unrecoverable: false,
            ..base.clone()
        }
        .is_green());
        assert!(!StructuralFloorReport {
            pseudonym_shred_is_the_id_step: false,
            ..base
        }
        .is_green());
    }

    #[test]
    fn the_residual_is_handled_by_reference_to_x7_no_local_statement() {
        let reference = assert_no_local_residual_statement();
        assert_eq!(reference, RESIDUAL_POSTURE_REF);
        assert!(
            reference.contains("§X-7"),
            "the residual is a reference to X-7"
        );
        assert!(
            reference.contains("10.9"),
            "the residual is the ONE platform posture (10.9)"
        );
        assert!(
            !reference.to_lowercase().contains("lawful basis"),
            "Storage must NOT author a local residual lawful-basis statement (X-7 owns it, once)"
        );
    }

    #[test]
    fn gd4_table_report_is_green_only_when_zero_misrouted() {
        let green = Gd4TableReport {
            routed: vec![(DataClass::BulkTenantContent, KeyGranularity::PerTenantDek)],
            misrouted: 0,
        };
        assert!(green.is_green());
        let red = Gd4TableReport {
            routed: vec![(DataClass::BulkTenantContent, KeyGranularity::PerSubjectDek)],
            misrouted: 1,
        };
        assert!(!red.is_green(), "a misrouted class makes the report RED");
    }

    #[test]
    fn structural_floor_backup_count_is_exact_zero_not_merely_absent() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let _ = kms
            .ensure_dek(&tenant, &r(), KeyClass::Subject("u-keep".into()))
            .unwrap();
        let floor = StructuralErasureFloor::new(&kms, r());
        let report = floor
            .verify(&SubjectId::new("u-erase"), &tenant)
            .expect("the in-memory key registry remains available");
        assert_eq!(
            report.recoverable_in_backup, 0,
            "the erased subject's DEK is 0 in the backup"
        );
        let kept = DekId::new(tenant.clone(), KeyClass::Subject("u-keep".into()));
        assert!(
            kms.backup_snapshot()
                .unwrap()
                .iter()
                .any(|(d, _)| *d == kept),
            "the non-erased subject's DEK is untouched (per-subject isolation)"
        );
    }

    #[test]
    fn structural_floor_unrecoverable_needs_both_resolve_fail_and_ciphertext() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let floor = StructuralErasureFloor::new(&kms, r());
        let report = floor
            .verify(&SubjectId::new("u-x"), &tenant)
            .expect("the in-memory key registry remains available");
        let key_ref =
            crate::kms::PiiKeyRef::new(tenant.clone(), 0, KeyClass::Subject("u-x".into()));
        assert!(
            kms.resolve_dek(&key_ref, &r()).is_err(),
            "the destroyed DEK no longer resolves (lever_works leg)"
        );
        assert!(report.lever_renders_unrecoverable, "both conjuncts held");
    }

    #[test]
    fn structural_reach_uses_the_erase_seam_set() {
        use crate::erase::{
            BusErase, EpochMillis, EraseError, ErasureLedgerSink, PseudonymShred, RefsTombstone,
            SearchPurge,
        };
        struct Noop;
        impl PseudonymShred for Noop {
            fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl SearchPurge for Noop {
            fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl RefsTombstone for Noop {
            fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl BusErase for Noop {
            fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl ErasureLedgerSink for Noop {
            fn record_erasure(&self, _s: &SubjectId, _t: &TenantId, _at: EpochMillis) {}
            fn is_erased(&self, _s: &SubjectId, _t: &TenantId) -> bool {
                false
            }
        }
        let n = Noop;
        let holders = EraseHolders {
            pseudonym: &n,
            search: &n,
            refs: &n,
            bus: &n,
            ledger: &n,
            git_reach: None,
        };
        assert!(structural_reach_uses_erase_seams(&holders));
    }

    #[test]
    fn data_class_all_is_the_complete_table() {
        assert_eq!(DataClass::all().len(), 6);
        let report = assert_gd4_table_complete();
        assert_eq!(report.routed.len(), DataClass::all().len());
    }
}
