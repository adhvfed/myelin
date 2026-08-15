//! Canonical coordinates shared by Git's storage, HTTP, CLI, and reference surfaces.

use std::fmt;

pub const MAX_REPOSITORY_SLUG_BYTES: usize = 255;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepositorySlug<'a>(&'a str);

impl<'a> RepositorySlug<'a> {
    pub fn parse(value: &'a str) -> Result<Self, RepositorySlugError> {
        if value.len() > MAX_REPOSITORY_SLUG_BYTES {
            return Err(RepositorySlugError::TooLong);
        }

        let mut segments = value.split('/').peekable();
        while let Some(segment) = segments.next() {
            if segment.is_empty() {
                return Err(RepositorySlugError::EmptySegment);
            }
            if matches!(segment, "." | "..") {
                return Err(RepositorySlugError::DotSegment);
            }
            if !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(RepositorySlugError::InvalidCharacter);
            }
            if segments.peek().is_some() && ends_with_git_case_insensitive(segment) {
                return Err(RepositorySlugError::BareRepositoryAncestor);
            }
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }

    pub fn segments(&self) -> impl DoubleEndedIterator<Item = &'a str> {
        self.0.split('/')
    }
}

impl<'a> TryFrom<&'a str> for RepositorySlug<'a> {
    type Error = RepositorySlugError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositorySlugError {
    TooLong,
    EmptySegment,
    DotSegment,
    InvalidCharacter,
    BareRepositoryAncestor,
}

impl fmt::Display for RepositorySlugError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(
                f,
                "repository slug exceeds the {MAX_REPOSITORY_SLUG_BYTES}-byte canonical bound"
            ),
            Self::EmptySegment => f.write_str("repository slug contains an empty segment"),
            Self::DotSegment => f.write_str("repository slug contains a path-traversal segment"),
            Self::InvalidCharacter => {
                f.write_str("repository slug contains a character outside [A-Za-z0-9._-]")
            }
            Self::BareRepositoryAncestor => f.write_str(
                "repository namespace segment ends in .git and would resolve inside a bare repository",
            ),
        }
    }
}

impl std::error::Error for RepositorySlugError {}

fn ends_with_git_case_insensitive(segment: &str) -> bool {
    segment
        .get(segment.len().saturating_sub(4)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".git"))
}

/// Parse the one canonical positive decimal spelling of a numeric coordinate.
pub fn parse_positive_decimal(value: &str) -> Option<u64> {
    let number = value.parse::<u64>().ok()?;
    (number > 0 && number.to_string() == value).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_slugs_have_one_storage_safe_shape() {
        let slug = RepositorySlug::parse("platform/api").unwrap();
        assert_eq!(slug.as_str(), "platform/api");
        assert_eq!(slug.segments().collect::<Vec<_>>(), ["platform", "api"]);
        assert!(RepositorySlug::parse("release.git").is_ok());

        for invalid in [
            "",
            "platform//api",
            "platform/../api",
            "platform.git/api",
            "PLATFORM.GIT/api",
            "platform/api#fragment",
            "platform\\api",
        ] {
            assert!(
                RepositorySlug::parse(invalid).is_err(),
                "invalid repository slug was admitted: {invalid}"
            );
        }
        assert!(RepositorySlug::parse(&"x".repeat(MAX_REPOSITORY_SLUG_BYTES + 1)).is_err());
    }

    #[test]
    fn positive_decimal_coordinates_have_one_spelling() {
        assert_eq!(parse_positive_decimal("42"), Some(42));
        assert_eq!(
            parse_positive_decimal(&u64::MAX.to_string()),
            Some(u64::MAX)
        );
        for invalid in [
            "",
            "0",
            "00",
            "01",
            "+1",
            "1.0",
            "1e2",
            "18446744073709551616",
        ] {
            assert_eq!(parse_positive_decimal(invalid), None, "{invalid}");
        }
    }
}
