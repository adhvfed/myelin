use core::fmt;

pub const MAX_AUTOMATION_TASK_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationTaskError {
    Empty,
    TooLarge,
    SurroundingWhitespace,
    UnsupportedControl,
}

impl fmt::Display for AutomationTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("task must not be empty"),
            Self::TooLarge => write!(
                formatter,
                "task must contain at most {MAX_AUTOMATION_TASK_BYTES} UTF-8 bytes"
            ),
            Self::SurroundingWhitespace => {
                formatter.write_str("task must not start or end with whitespace")
            }
            Self::UnsupportedControl => formatter.write_str(
                "task may contain line breaks and tabs, but no other control characters",
            ),
        }
    }
}

impl std::error::Error for AutomationTaskError {}

pub fn validate_automation_task(task: &str) -> Result<(), AutomationTaskError> {
    if task.is_empty() {
        return Err(AutomationTaskError::Empty);
    }
    if task.len() > MAX_AUTOMATION_TASK_BYTES {
        return Err(AutomationTaskError::TooLarge);
    }
    if task.trim() != task {
        return Err(AutomationTaskError::SurroundingWhitespace);
    }
    if task
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AutomationTaskError::UnsupportedControl);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_tasks_are_bounded_readable_multiline_prompts() {
        assert_eq!(
            validate_automation_task(""),
            Err(AutomationTaskError::Empty)
        );
        assert_eq!(
            validate_automation_task(" describe the work"),
            Err(AutomationTaskError::SurroundingWhitespace)
        );
        assert_eq!(
            validate_automation_task("describe\0the work"),
            Err(AutomationTaskError::UnsupportedControl)
        );
        assert_eq!(
            validate_automation_task("describe\u{1b}[31mthe work"),
            Err(AutomationTaskError::UnsupportedControl)
        );
        assert_eq!(
            validate_automation_task(&"é".repeat(MAX_AUTOMATION_TASK_BYTES / 2 + 1)),
            Err(AutomationTaskError::TooLarge)
        );
        assert!(
            validate_automation_task("Inspect the failure.\n\tOpen one focused issue.").is_ok()
        );
    }
}
