use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::EdgeError;

const MAX_WEB_COMMIT_MESSAGE_BYTES: usize = 8 * 1024;
const DEFAULT_WEB_COMMIT_MESSAGE: &str = "web edit";

pub(super) fn required_json<T: DeserializeOwned>(
    bytes: &[u8],
    mutation: &str,
) -> Result<T, EdgeError> {
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(format!(
            "{mutation} request body is empty"
        )));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid {mutation} request: {error}")))
}

pub(super) fn optional_json<T: Default + DeserializeOwned>(
    bytes: &[u8],
    mutation: &str,
) -> Result<T, EdgeError> {
    if bytes.is_empty() {
        Ok(T::default())
    } else {
        required_json(bytes, mutation)
    }
}

pub(super) fn canonical_json<T: Serialize>(request: T) -> Result<Value, EdgeError> {
    serde_json::to_value(request)
        .map_err(|error| EdgeError::Internal(format!("encode validated Git request: {error}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateRepositoryBody {
    #[serde(alias = "name")]
    pub(super) slug: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WebEditCommitBody {
    pub(super) base_oid: String,
    pub(super) contents: String,
    pub(super) start_ref: Option<String>,
    pub(super) message: Option<String>,
}

impl WebEditCommitBody {
    pub(super) fn commit_message(&self) -> Result<&str, EdgeError> {
        let Some(message) = self.message.as_deref() else {
            return Ok(DEFAULT_WEB_COMMIT_MESSAGE);
        };
        if message.trim().is_empty() {
            return Err(EdgeError::BadRequest(
                "file commit message must not be blank".into(),
            ));
        }
        if message.len() > MAX_WEB_COMMIT_MESSAGE_BYTES {
            return Err(EdgeError::BadRequest(format!(
                "file commit message exceeds {MAX_WEB_COMMIT_MESSAGE_BYTES} bytes"
            )));
        }
        if message.contains('\0') {
            return Err(EdgeError::BadRequest(
                "file commit message must not contain a null byte".into(),
            ));
        }
        Ok(message)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenPullRequestBody {
    pub(super) title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) head_ref: Option<String>,
    #[serde(alias = "head_repo_slug", skip_serializing_if = "Option::is_none")]
    pub(super) head_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) head_oid: Option<String>,
    #[serde(alias = "body", skip_serializing_if = "Option::is_none")]
    pub(super) body_md: Option<String>,
    #[serde(default)]
    pub(super) draft: bool,
    #[serde(default)]
    pub(super) reviewers: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ThreadCreateBody {
    #[serde(alias = "body")]
    pub(super) body_md: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) anchor: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommentBody {
    #[serde(alias = "body")]
    pub(super) body_md: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolveThreadBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) resolved: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubmitReviewBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) summary_md: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewBody {
    pub(super) verdict: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyMutationBody {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_creation_has_one_unambiguous_name() {
        let canonical: CreateRepositoryBody =
            required_json(br#"{"slug":"platform"}"#, "repository creation").unwrap();
        assert_eq!(canonical.slug, "platform");

        let compatible: CreateRepositoryBody =
            required_json(br#"{"name":"platform"}"#, "repository creation").unwrap();
        assert_eq!(compatible.slug, "platform");

        for ambiguous in [
            br#"{"slug":"platform","name":"other"}"#.as_slice(),
            br#"{"slug":"platform","ignored":true}"#.as_slice(),
        ] {
            assert!(
                required_json::<CreateRepositoryBody>(ambiguous, "repository creation").is_err()
            );
        }
    }

    #[test]
    fn pull_request_input_rejects_misspellings_and_wrong_types() {
        let canonical: OpenPullRequestBody = required_json(
            br#"{"title":"Ship it","body":"The reason","draft":true}"#,
            "pull request creation",
        )
        .unwrap();
        assert_eq!(canonical.body_md.as_deref(), Some("The reason"));
        assert!(canonical.draft);

        for malformed in [
            br#"{"title":"Ship it","target_ref":"main"}"#.as_slice(),
            br#"{"title":"Ship it","draft":"false"}"#.as_slice(),
            br#"{"title":"Ship it","body":"one","body_md":"two"}"#.as_slice(),
        ] {
            assert!(
                required_json::<OpenPullRequestBody>(malformed, "pull request creation").is_err()
            );
        }
    }

    #[test]
    fn file_commits_keep_a_bounded_explicit_message_or_use_the_clear_default() {
        let explicit: WebEditCommitBody = required_json(
            br#"{"base_oid":"","contents":"hello","message":"Explain the change"}"#,
            "file commit",
        )
        .unwrap();
        assert_eq!(explicit.commit_message().unwrap(), "Explain the change");

        let defaulted: WebEditCommitBody =
            required_json(br#"{"base_oid":"","contents":"hello"}"#, "file commit").unwrap();
        assert_eq!(defaulted.commit_message().unwrap(), "web edit");

        for invalid in [
            br#"{"base_oid":"","contents":"hello","message":"  "}"#.as_slice(),
            br#"{"base_oid":"","contents":"hello","message":"contains\u0000null"}"#.as_slice(),
        ] {
            let request: WebEditCommitBody = required_json(invalid, "file commit").unwrap();
            assert!(request.commit_message().is_err());
        }
    }

    #[test]
    fn optional_mutations_accept_only_an_empty_object_or_no_body() {
        assert!(optional_json::<EmptyMutationBody>(&[], "merge").is_ok());
        assert!(optional_json::<EmptyMutationBody>(br#"{}"#, "merge").is_ok());
        assert!(optional_json::<EmptyMutationBody>(br#"{"force":true}"#, "merge").is_err());
    }

    #[test]
    fn review_inputs_do_not_turn_wrong_types_into_defaults() {
        assert!(optional_json::<ResolveThreadBody>(
            br#"{"resolved":"false"}"#,
            "thread resolution"
        )
        .is_err());
        assert!(
            optional_json::<SubmitReviewBody>(br#"{"verdict":false}"#, "review submission")
                .is_err()
        );
    }
}
