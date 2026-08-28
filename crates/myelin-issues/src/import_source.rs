#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceSystem {
    Jira,
    Linear,
    GitHub,
    Csv,
}

impl SourceSystem {
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "jira" => Some(Self::Jira),
            "linear" => Some(Self::Linear),
            "github" => Some(Self::GitHub),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }

    pub const fn token(self) -> &'static str {
        match self {
            Self::Jira => "jira",
            Self::Linear => "linear",
            Self::GitHub => "github",
            Self::Csv => "csv",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_import_source_tokens_are_total_and_case_sensitive() {
        for (token, source) in [
            ("jira", SourceSystem::Jira),
            ("linear", SourceSystem::Linear),
            ("github", SourceSystem::GitHub),
            ("csv", SourceSystem::Csv),
        ] {
            assert_eq!(SourceSystem::parse(token), Some(source));
            assert_eq!(source.token(), token);
        }
        for invalid in ["", "Jira", "git_hub", "unknown"] {
            assert_eq!(SourceSystem::parse(invalid), None);
        }
    }
}
