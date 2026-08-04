use myelin_gdpr::PersonalData;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

#[derive(PersonalData)]
pub struct WorkflowRunRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub wf_type: String,
    pub wf_version: i32,
    pub input: Vec<ArtifactRef>,
    pub state: String,
    pub cursor: i64,
    pub budget_json: Option<String>,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub caused_by: Option<String>,
    pub depth: i32,
    pub partition: i16,
    pub lease_owner: Option<String>,
    pub lease_expires: Option<String>,
}

#[derive(PersonalData, Clone, Debug, PartialEq)]
pub struct WfHistoryRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub seq: i64,
    pub kind: String,
    pub command_id: String,
    pub result: Option<Vec<ArtifactRef>>,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "result_key_ref",
    )]
    pub result_key_ref: Option<String>,
}

#[derive(PersonalData)]
pub struct WfTimerRow {
    pub tenant: TenantId,
    pub region: Region,
    pub timer_id: String,
    pub run_id: Option<String>,
    pub command_id: String,
    pub fire_at: String,
    pub bucket: i32,
    pub fired: bool,
    pub partition: i16,
}

#[derive(PersonalData)]
pub struct WfSignalRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub signal_name: String,
    pub idem_key: String,
    pub payload: Vec<ArtifactRef>,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "payload_key_ref",
    )]
    pub payload_key_ref: Option<String>,
    pub consumed_seq: Option<i64>,
}

#[derive(PersonalData, Clone, Debug, PartialEq)]
pub struct WfActivityAttemptRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub command_id: String,
    pub attempt: i32,
    pub idem_token: String,
    pub state: String,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

#[derive(PersonalData)]
pub struct WfDefinitionRow {
    pub wf_type: String,
    pub version: i32,
    pub code_hash: String,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> TenantId {
        TenantId::from_token("acme")
    }
    fn r() -> Region {
        Region::new("fr-par")
    }

    #[test]
    fn the_six_tables_compile_with_tags_and_tenant_first_keys() {
        let run = WorkflowRunRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            wf_type: "agent.run".into(),
            wf_version: 1,
            input: vec![ArtifactRef("myelin://acme/git/pr/PR-1".into())],
            state: "running".into(),
            cursor: 0,
            budget_json: Some("{\"minor_units\":10000}".into()),
            correlation_id: "corr-1".into(),
            causation_id: Some("evt-1".into()),
            caused_by: Some("sess-1".into()),
            depth: 0,
            partition: 3,
            lease_owner: None,
            lease_expires: None,
        };
        assert_eq!(run.tenant, t());
        assert_eq!(run.region, r());
        let _input: &Vec<ArtifactRef> = &run.input;
        assert_eq!(run.state, "running");
        assert_eq!(run.cursor, 0);

        let history = WfHistoryRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            seq: 1,
            kind: "activity_completed".into(),
            command_id: "agent.run:0".into(),
            result: Some(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())]),
            result_key_ref: Some("kms://acme/subject/u1".into()),
        };
        let _result: &Option<Vec<ArtifactRef>> = &history.result;
        assert!(history.result_key_ref.is_some());
        assert_eq!(history.command_id, "agent.run:0");

        let timer = WfTimerRow {
            tenant: t(),
            region: r(),
            timer_id: "tmr-1".into(),
            run_id: Some("01J-run".into()),
            command_id: "agent.run:1".into(),
            fire_at: "2026-07-21T00:00:00Z".into(),
            bucket: 29_000_000,
            fired: false,
            partition: 3,
        };
        assert!(!timer.fired);

        let signal = WfSignalRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            signal_name: "job.done".into(),
            idem_key: "tok-1".into(),
            payload: vec![ArtifactRef("myelin://acme/ci/job/J1".into())],
            payload_key_ref: None,
            consumed_seq: None,
        };
        assert_eq!(signal.idem_key, "tok-1");
        let _payload: &Vec<ArtifactRef> = &signal.payload;

        let attempt = WfActivityAttemptRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            command_id: "agent.run:0".into(),
            attempt: 1,
            idem_token: "tok-1".into(),
            state: "succeeded".into(),
            error: None,
            started_at: Some("2026-06-21T00:00:00Z".into()),
            ended_at: Some("2026-06-21T00:00:01Z".into()),
        };
        assert_eq!(attempt.idem_token, "tok-1");

        let def = WfDefinitionRow {
            wf_type: "agent.run".into(),
            version: 1,
            code_hash: "blake3:deadbeef".into(),
            status: "active".into(),
        };
        assert_eq!(def.version, 1);
        assert_eq!(def.status, "active");
    }
}
