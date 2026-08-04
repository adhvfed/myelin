use myelin_events::check_seam::{rollup_ci_result, CiOverall, CiResult};
use myelin_flow::{encode_ci_result, SignalRow, SignalStore, CI_RESULT_SIGNAL};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;

pub struct CiResultSignal<'a> {
    signals: &'a SignalStore,
    tenant: TenantId,
    region: Region,
    merge_queue_run: String,
}

impl<'a> CiResultSignal<'a> {
    pub fn new(
        signals: &'a SignalStore,
        tenant: TenantId,
        region: Region,
        merge_queue_run: impl Into<String>,
    ) -> CiResultSignal<'a> {
        CiResultSignal {
            signals,
            tenant,
            region,
            merge_queue_run: merge_queue_run.into(),
        }
    }

    pub fn rollup(
        &self,
        commit_oid: &str,
        current: &BTreeMap<String, bool>,
        required: &[String],
        idem_token: &str,
    ) -> CiResult {
        rollup_ci_result(commit_oid, current, required, idem_token)
    }

    pub fn signal_ci_result(
        &self,
        commit_oid: &str,
        current: &BTreeMap<String, bool>,
        required: &[String],
        idem_token: &str,
    ) -> RollupDelivery {
        let result = self.rollup(commit_oid, current, required, idem_token);
        self.deliver(&result)
    }

    pub fn deliver(&self, result: &CiResult) -> RollupDelivery {
        let row = SignalRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.merge_queue_run.clone(),
            signal_name: CI_RESULT_SIGNAL.to_string(),
            idem_key: result.idem_token.clone(),
            payload: encode_ci_result(result),
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        };
        if self.signals.deliver(row) {
            RollupDelivery::Woke
        } else {
            RollupDelivery::Duplicate
        }
    }

    pub fn is_success(result: &CiResult) -> bool {
        result.overall == CiOverall::Success
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollupDelivery {
    Woke,
    Duplicate,
}

#[cfg(test)]
#[path = "ci_result_signal_tests.rs"]
mod tests;
