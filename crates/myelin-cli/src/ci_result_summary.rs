use serde_json::{Map, Value};

const MAX_CI_DIAGNOSTIC_BYTES: usize = 2_048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CiJobDisposition {
    WorkloadPassed,
    WorkloadFailed,
    WorkloadTimedOut,
    SecretResolutionFailed,
    SecretResolutionTimedOut,
    CheckoutTransportFailed,
    CheckoutTransportTimedOut,
    CheckoutMaterializationFailed,
    CheckoutMaterializationTimedOut,
    PreparationAttemptsExhausted,
    SkippedBeforeStart,
    CancelledDuringPreparation,
    CancelledAfterWorkloadLaunch,
    ConfigurationRefused,
}

impl CiJobDisposition {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "workload_passed" => Self::WorkloadPassed,
            "workload_failed" => Self::WorkloadFailed,
            "workload_timed_out" => Self::WorkloadTimedOut,
            "secret_resolution_failed" => Self::SecretResolutionFailed,
            "secret_resolution_timed_out" => Self::SecretResolutionTimedOut,
            "checkout_transport_failed" => Self::CheckoutTransportFailed,
            "checkout_transport_timed_out" => Self::CheckoutTransportTimedOut,
            "checkout_materialization_failed" => Self::CheckoutMaterializationFailed,
            "checkout_materialization_timed_out" => Self::CheckoutMaterializationTimedOut,
            "preparation_attempts_exhausted" => Self::PreparationAttemptsExhausted,
            "skipped_before_start" => Self::SkippedBeforeStart,
            "cancelled_during_preparation" => Self::CancelledDuringPreparation,
            "cancelled_after_workload_launch" => Self::CancelledAfterWorkloadLaunch,
            "configuration_refused" => Self::ConfigurationRefused,
            _ => return None,
        })
    }

    fn facts(self) -> (bool, bool, bool) {
        match self {
            Self::WorkloadPassed => (true, false, true),
            Self::WorkloadFailed | Self::CancelledAfterWorkloadLaunch => (false, false, true),
            Self::WorkloadTimedOut => (false, true, true),
            Self::SecretResolutionTimedOut
            | Self::CheckoutTransportTimedOut
            | Self::CheckoutMaterializationTimedOut => (false, true, false),
            Self::SecretResolutionFailed
            | Self::CheckoutTransportFailed
            | Self::CheckoutMaterializationFailed
            | Self::PreparationAttemptsExhausted
            | Self::SkippedBeforeStart
            | Self::CancelledDuringPreparation
            | Self::ConfigurationRefused => (false, false, false),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::WorkloadPassed => "Workload passed",
            Self::WorkloadFailed => "Workload failed",
            Self::WorkloadTimedOut => "Workload timed out",
            Self::SecretResolutionFailed => "Secret resolution failed",
            Self::SecretResolutionTimedOut => "Secret resolution timed out",
            Self::CheckoutTransportFailed => "Repository checkout failed",
            Self::CheckoutTransportTimedOut => "Repository checkout timed out",
            Self::CheckoutMaterializationFailed => "Checkout verification failed",
            Self::CheckoutMaterializationTimedOut => "Checkout verification timed out",
            Self::PreparationAttemptsExhausted => "Checkout attempts exhausted",
            Self::SkippedBeforeStart => "Skipped before start",
            Self::CancelledDuringPreparation => "Cancelled during preparation",
            Self::CancelledAfterWorkloadLaunch => "Cancelled while running",
            Self::ConfigurationRefused => "Pipeline configuration refused",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CiJobResultSummary<'a> {
    passed: bool,
    timed_out: bool,
    disposition: Option<CiJobDisposition>,
    workload_started: Option<bool>,
    diagnostic: Option<&'a str>,
}

impl<'a> CiJobResultSummary<'a> {
    pub(crate) fn label(self) -> &'static str {
        self.disposition.map_or_else(
            || {
                if self.timed_out {
                    "Timed out"
                } else if self.passed {
                    "Passed"
                } else {
                    "Failed"
                }
            },
            CiJobDisposition::label,
        )
    }

    pub(crate) fn diagnostic(self) -> Option<&'a str> {
        self.diagnostic
    }

    pub(crate) fn workload_started(self) -> Option<bool> {
        self.workload_started
    }
}

pub(crate) fn parse_ci_job_result_summary(
    value: &Value,
) -> Result<Option<CiJobResultSummary<'_>>, &'static str> {
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or("run job result summary must be an object or null")?;
    if exact(object, &["passed", "timed_out"]) {
        let passed = boolean(object, "passed")?;
        let timed_out = boolean(object, "timed_out")?;
        if passed && timed_out {
            return Err("legacy run job result cannot both pass and time out");
        }
        return Ok(Some(CiJobResultSummary {
            passed,
            timed_out,
            disposition: None,
            workload_started: None,
            diagnostic: None,
        }));
    }

    let has_diagnostic = object.contains_key("diagnostic");
    let expected_fields = if has_diagnostic {
        &[
            "passed",
            "timed_out",
            "disposition",
            "workload_started",
            "diagnostic",
        ][..]
    } else {
        &["passed", "timed_out", "disposition", "workload_started"][..]
    };
    if !exact(object, expected_fields) {
        return Err("run job result summary has missing or unknown fields");
    }
    let passed = boolean(object, "passed")?;
    let timed_out = boolean(object, "timed_out")?;
    let workload_started = boolean(object, "workload_started")?;
    let disposition = object["disposition"]
        .as_str()
        .and_then(CiJobDisposition::parse)
        .ok_or("run job result disposition is invalid")?;
    if (passed, timed_out, workload_started) != disposition.facts() {
        return Err("run job result facts contradict its disposition");
    }
    let diagnostic = if has_diagnostic {
        Some(
            object["diagnostic"]
                .as_str()
                .filter(|value| bounded_diagnostic(value))
                .ok_or("run job result diagnostic is not bounded display text")?,
        )
    } else {
        None
    };
    Ok(Some(CiJobResultSummary {
        passed,
        timed_out,
        disposition: Some(disposition),
        workload_started: Some(workload_started),
        diagnostic,
    }))
}

fn exact(object: &Map<String, Value>, fields: &[&str]) -> bool {
    object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
}

fn boolean(object: &Map<String, Value>, field: &str) -> Result<bool, &'static str> {
    object[field]
        .as_bool()
        .ok_or("run job result facts must be booleans")
}

fn bounded_diagnostic(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CI_DIAGNOSTIC_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modern_summary_is_fact_checked_and_humanized() {
        let value = json!({
            "passed": false,
            "timed_out": false,
            "disposition": "configuration_refused",
            "workload_started": false,
            "diagnostic": "run-plan schema V2 is required",
        });
        let summary = parse_ci_job_result_summary(&value).unwrap().unwrap();
        assert_eq!(summary.label(), "Pipeline configuration refused");
        assert_eq!(summary.diagnostic(), Some("run-plan schema V2 is required"));
        assert_eq!(summary.workload_started(), Some(false));

        assert!(parse_ci_job_result_summary(&json!({
            "passed": false,
            "timed_out": false,
            "disposition": "workload_failed",
            "workload_started": false,
        }))
        .is_err());
        assert!(parse_ci_job_result_summary(&json!({
            "passed": false,
            "timed_out": false,
            "disposition": "configuration_refused",
            "workload_started": false,
            "diagnostic": "first\u{2028}second",
        }))
        .is_err());
    }

    #[test]
    fn legacy_summary_remains_readable_but_arbitrary_json_does_not() {
        let value = json!({
            "passed": false,
            "timed_out": false,
        });
        let legacy = parse_ci_job_result_summary(&value).unwrap().unwrap();
        assert_eq!(legacy.label(), "Failed");
        assert_eq!(legacy.diagnostic(), None);
        assert_eq!(legacy.workload_started(), None);
        assert!(parse_ci_job_result_summary(&json!({ "message": "failed" })).is_err());
    }
}
