use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::EdgeError;

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateRepositoryBody {
    #[serde(alias = "name")]
    pub(super) slug: String,
}

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
}
