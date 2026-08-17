use serde_json::{Map, Value};

const MAX_CI_DIAGNOSTIC_BYTES: usize = 2_048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CiJobDisposition {
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
    pub(crate) fn from_token(value: &str) -> Option<Self> {
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

    fn token(self) -> &'static str {
        match self {
            Self::WorkloadPassed => "workload_passed",
            Self::WorkloadFailed => "workload_failed",
            Self::WorkloadTimedOut => "workload_timed_out",
            Self::SecretResolutionFailed => "secret_resolution_failed",
            Self::SecretResolutionTimedOut => "secret_resolution_timed_out",
            Self::CheckoutTransportFailed => "checkout_transport_failed",
            Self::CheckoutTransportTimedOut => "checkout_transport_timed_out",
            Self::CheckoutMaterializationFailed => "checkout_materialization_failed",
            Self::CheckoutMaterializationTimedOut => "checkout_materialization_timed_out",
            Self::PreparationAttemptsExhausted => "preparation_attempts_exhausted",
            Self::SkippedBeforeStart => "skipped_before_start",
            Self::CancelledDuringPreparation => "cancelled_during_preparation",
            Self::CancelledAfterWorkloadLaunch => "cancelled_after_workload_launch",
            Self::ConfigurationRefused => "configuration_refused",
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobResultSummary {
    shape: CiJobResultShape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CiJobResultShape {
    Legacy {
        passed: bool,
        timed_out: bool,
    },
    Current {
        disposition: CiJobDisposition,
        diagnostic: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiJobResultError(&'static str);

impl core::fmt::Display for CiJobResultError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for CiJobResultError {}

impl CiJobResultSummary {
    pub(crate) fn legacy(passed: bool, timed_out: bool) -> Result<Self, CiJobResultError> {
        if passed && timed_out {
            return Err(CiJobResultError(
                "legacy run job result cannot both pass and time out",
            ));
        }
        Ok(Self {
            shape: CiJobResultShape::Legacy { passed, timed_out },
        })
    }

    pub(crate) fn current(disposition: CiJobDisposition, diagnostic: Option<&str>) -> Self {
        let diagnostic = diagnostic
            .map(canonical_ci_diagnostic)
            .filter(|value| !value.is_empty());
        Self {
            shape: CiJobResultShape::Current {
                disposition,
                diagnostic,
            },
        }
    }

    pub fn parse(value: &Value) -> Result<Option<Self>, CiJobResultError> {
        if value.is_null() {
            return Ok(None);
        }
        let object = value.as_object().ok_or(CiJobResultError(
            "run job result summary must be an object or null",
        ))?;
        if exact(object, &["passed", "timed_out"]) {
            return Self::legacy(boolean(object, "passed")?, boolean(object, "timed_out")?)
                .map(Some);
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
            return Err(CiJobResultError(
                "run job result summary has missing or unknown fields",
            ));
        }
        let passed = boolean(object, "passed")?;
        let timed_out = boolean(object, "timed_out")?;
        let workload_started = boolean(object, "workload_started")?;
        let disposition = object["disposition"]
            .as_str()
            .and_then(CiJobDisposition::from_token)
            .ok_or(CiJobResultError("run job result disposition is invalid"))?;
        if (passed, timed_out, workload_started) != disposition.facts() {
            return Err(CiJobResultError(
                "run job result facts contradict its disposition",
            ));
        }
        let diagnostic = if has_diagnostic {
            Some(
                object["diagnostic"]
                    .as_str()
                    .filter(|value| canonical_diagnostic(value))
                    .ok_or(CiJobResultError(
                        "run job result diagnostic is not bounded display text",
                    ))?,
            )
        } else {
            None
        };
        Ok(Some(Self {
            shape: CiJobResultShape::Current {
                disposition,
                diagnostic: diagnostic.map(str::to_owned),
            },
        }))
    }

    pub(crate) fn passed(&self) -> bool {
        match self.shape {
            CiJobResultShape::Legacy { passed, .. } => passed,
            CiJobResultShape::Current { disposition, .. } => disposition.facts().0,
        }
    }

    pub(crate) fn timed_out(&self) -> bool {
        match self.shape {
            CiJobResultShape::Legacy { timed_out, .. } => timed_out,
            CiJobResultShape::Current { disposition, .. } => disposition.facts().1,
        }
    }

    pub fn workload_started(&self) -> Option<bool> {
        match self.shape {
            CiJobResultShape::Legacy { .. } => None,
            CiJobResultShape::Current { disposition, .. } => Some(disposition.facts().2),
        }
    }

    pub fn label(&self) -> &'static str {
        match self.shape {
            CiJobResultShape::Legacy { passed, timed_out } => {
                if timed_out {
                    "Timed out"
                } else if passed {
                    "Passed"
                } else {
                    "Failed"
                }
            }
            CiJobResultShape::Current { disposition, .. } => disposition.label(),
        }
    }

    pub fn diagnostic(&self) -> Option<&str> {
        match &self.shape {
            CiJobResultShape::Legacy { .. } => None,
            CiJobResultShape::Current { diagnostic, .. } => diagnostic.as_deref(),
        }
    }

    pub fn to_value(&self) -> Value {
        match &self.shape {
            CiJobResultShape::Legacy { passed, timed_out } => serde_json::json!({
                "passed": passed,
                "timed_out": timed_out,
            }),
            CiJobResultShape::Current {
                disposition,
                diagnostic,
            } => {
                let (passed, timed_out, workload_started) = disposition.facts();
                let mut value = serde_json::json!({
                    "passed": passed,
                    "timed_out": timed_out,
                    "disposition": disposition.token(),
                    "workload_started": workload_started,
                });
                if let Some(diagnostic) = diagnostic {
                    value["diagnostic"] = Value::String(diagnostic.clone());
                }
                value
            }
        }
    }
}

pub(crate) fn canonical_ci_diagnostic(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(MAX_CI_DIAGNOSTIC_BYTES));
    for character in value.chars() {
        let character = if unsafe_display_character(character) {
            '�'
        } else {
            character
        };
        if output.len() + character.len_utf8() > MAX_CI_DIAGNOSTIC_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

fn exact(object: &Map<String, Value>, fields: &[&str]) -> bool {
    object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
}

fn boolean(object: &Map<String, Value>, field: &str) -> Result<bool, CiJobResultError> {
    object[field]
        .as_bool()
        .ok_or(CiJobResultError("run job result facts must be booleans"))
}

fn canonical_diagnostic(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CI_DIAGNOSTIC_BYTES
        && !value.chars().any(unsafe_display_character)
}

fn unsafe_display_character(character: char) -> bool {
    character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn current_summary_is_canonical_fact_checked_and_humanized() {
        let summary = CiJobResultSummary::current(
            CiJobDisposition::ConfigurationRefused,
            Some("run-plan schema V2 is required"),
        );
        assert_eq!(summary.label(), "Pipeline configuration refused");
        assert_eq!(summary.diagnostic(), Some("run-plan schema V2 is required"));
        assert_eq!(summary.workload_started(), Some(false));
        assert_eq!(
            CiJobResultSummary::parse(&summary.to_value()),
            Ok(Some(summary))
        );

        assert!(CiJobResultSummary::parse(&json!({
            "passed": false,
            "timed_out": false,
            "disposition": "workload_failed",
            "workload_started": false,
        }))
        .is_err());

        let without_diagnostic =
            CiJobResultSummary::current(CiJobDisposition::WorkloadFailed, Some(""));
        assert!(!without_diagnostic
            .to_value()
            .as_object()
            .unwrap()
            .contains_key("diagnostic"));
        assert!(CiJobResultSummary::parse(&json!({
            "passed": false,
            "timed_out": false,
            "disposition": "configuration_refused",
            "workload_started": false,
            "diagnostic": "first\u{2028}second",
        }))
        .is_err());
    }

    #[test]
    fn legacy_summary_remains_exact_but_arbitrary_json_does_not() {
        let legacy = CiJobResultSummary::legacy(false, false).unwrap();
        assert_eq!(legacy.label(), "Failed");
        assert_eq!(legacy.diagnostic(), None);
        assert_eq!(legacy.workload_started(), None);
        assert_eq!(
            CiJobResultSummary::parse(&legacy.to_value()),
            Ok(Some(legacy))
        );
        assert!(CiJobResultSummary::parse(&json!({ "message": "failed" })).is_err());
        assert!(CiJobResultSummary::legacy(true, true).is_err());
    }

    #[test]
    fn diagnostics_are_single_line_utf8_bounded_text() {
        let input = format!(
            "checkout\n\u{85}\u{2028}\u{2029}{}\u{7f}",
            "é".repeat(MAX_CI_DIAGNOSTIC_BYTES)
        );
        let bounded = canonical_ci_diagnostic(&input);

        assert!(bounded.len() <= MAX_CI_DIAGNOSTIC_BYTES);
        assert!(!bounded.chars().any(unsafe_display_character));
        assert!(bounded.starts_with("checkout����"));
    }
}
