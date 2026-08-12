use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    assert_gd4_table_complete, assert_no_local_residual_statement, key_choice_granularity,
    DataClass, KekId, KeyGranularity, KmsEngine, StructuralErasureFloor, SubjectId,
    RESIDUAL_POSTURE_REF,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}

#[test]
fn gd4_granularity_gate_zero_misrouted_classes_and_structural_floor_holds() {
    let table = assert_gd4_table_complete();
    assert_eq!(
        table.misrouted, 0,
        "GD-4 GATE RED: a data class routed to the WRONG granularity (the §5.1 table is the rule, \
         the threshold is 0 misrouted and is NOT weakened)"
    );
    assert!(table.is_green());
    assert_eq!(
        table.routed.len(),
        6,
        "the §5.1 table must be COMPLETE (all six classes)"
    );

    for (class, granularity) in &table.routed {
        let expected = match class {
            DataClass::FreeTextProfile
            | DataClass::ChatBody
            | DataClass::AgentMemory
            | DataClass::CiInlinePiiLog => KeyGranularity::PerSubjectDek,
            DataClass::BulkTenantContent => KeyGranularity::PerTenantDek,
            DataClass::TenantOffboard => KeyGranularity::PerTenantKek,
        };
        assert_eq!(*granularity, expected, "class {class:?} misrouted");
    }

    assert_eq!(
        key_choice_granularity(
            &ErasureMethod::CryptoShred("subject_dek".into()),
            Some(&SubjectId::new("u-1")),
        )
        .unwrap(),
        KeyGranularity::PerSubjectDek
    );
    assert_eq!(
        key_choice_granularity(&ErasureMethod::PurgeReindex, None).unwrap(),
        KeyGranularity::PerTenantDek
    );

    let tenant = TenantId("acme".into());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let floor = StructuralErasureFloor::new(&kms, region());
    let report = floor
        .verify(&SubjectId::new("u-structural"), &tenant)
        .expect("the in-memory key registry remains available");
    assert!(
        report.lever_renders_unrecoverable,
        "STRUCTURAL FLOOR RED: the per-subject DEK shred did not render the subject unrecoverable"
    );
    assert_eq!(
        report.recoverable_in_backup, 0,
        "STRUCTURAL FLOOR RED: the destroyed subject DEK is STILL in a backup (crypto-shred must \
         reach backups by construction, §7.5 - threshold 0, NOT weakened)"
    );
    assert!(report.pseudonym_shred_is_the_id_step);
    assert!(
        report.is_green(),
        "the structural GDPR floor holds (all three guarantees)"
    );

    let residual = assert_no_local_residual_statement();
    assert_eq!(residual, RESIDUAL_POSTURE_REF);
    assert!(
        residual.contains("§X-7") && residual.contains("10.9"),
        "the residual is a REFERENCE to the ONE platform posture (X-7 / 10.9), not a local statement"
    );
    assert!(
        !residual.to_lowercase().contains("lawful basis"),
        "Storage must NOT author a local residual lawful-basis statement (X-7 owns it, once)"
    );

    println!(
        "GD-4 GRANULARITY GATE GREEN [2026-06-20] misrouted_classes={} (threshold 0), classes={} \
         (free-text/chat/agent-memory/ci-inline-pii→subject-DEK, bulk→tenant-DEK, offboard→tenant-KEK); \
         structural-floor: lever_unrecoverable={} recoverable_in_backup={} (threshold 0) \
         pseudonym_shred=id-step; residual=BY-REFERENCE→X-7/10.9 (0 Storage-local residual statements)",
        table.misrouted,
        table.routed.len(),
        report.lever_renders_unrecoverable,
        report.recoverable_in_backup,
    );
}
