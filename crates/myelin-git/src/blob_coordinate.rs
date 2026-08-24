use myelin_events::{ArtifactRef, SubjectComponent};

use crate::coordinate::RepositorySlug;
use crate::core::is_canonical_object_id;
use crate::receive_pack::RefName;
use crate::refs_pagination::RefKind;

pub const MAX_BLOB_PATH_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitBlobLocation {
    ref_name: String,
    path: String,
}

impl GitBlobLocation {
    pub fn new(ref_name: &str, path: &str) -> Result<Self, GitBlobEventKeyError> {
        validate_blob_location(ref_name, path)?;
        Ok(Self {
            ref_name: ref_name.to_owned(),
            path: path.to_owned(),
        })
    }

    pub fn ref_name(&self) -> &str {
        &self.ref_name
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitBlobEventKey {
    encoded_repository: SubjectComponent,
    encoded_ref: SubjectComponent,
    encoded_path: SubjectComponent,
}

impl GitBlobEventKey {
    pub fn new(repository: &str, ref_name: &str, path: &str) -> Result<Self, GitBlobEventKeyError> {
        RepositorySlug::parse(repository).map_err(|_| GitBlobEventKeyError)?;
        GitBlobLocation::new(ref_name, path)?;
        Ok(Self {
            encoded_repository: SubjectComponent::encode(repository)
                .map_err(|_| GitBlobEventKeyError)?,
            encoded_ref: SubjectComponent::encode(ref_name).map_err(|_| GitBlobEventKeyError)?,
            encoded_path: SubjectComponent::encode(path).map_err(|_| GitBlobEventKeyError)?,
        })
    }

    pub fn parse_id(id: &str) -> Result<(String, GitBlobLocation), GitBlobEventKeyError> {
        let mut components = id.split(':');
        let repository = components.next().ok_or(GitBlobEventKeyError)?;
        let ref_name = components.next().ok_or(GitBlobEventKeyError)?;
        let path = components.next().ok_or(GitBlobEventKeyError)?;
        if components.next().is_some() {
            return Err(GitBlobEventKeyError);
        }
        let repository = SubjectComponent::parse(repository)
            .map_err(|_| GitBlobEventKeyError)?
            .decode();
        let ref_name = SubjectComponent::parse(ref_name)
            .map_err(|_| GitBlobEventKeyError)?
            .decode();
        let path = SubjectComponent::parse(path)
            .map_err(|_| GitBlobEventKeyError)?
            .decode();
        RepositorySlug::parse(&repository).map_err(|_| GitBlobEventKeyError)?;
        Ok((repository, GitBlobLocation::new(&ref_name, &path)?))
    }

    pub fn subject(&self, tenant: &str) -> Result<ArtifactRef, GitBlobEventKeyError> {
        myelin_refs::parse(&format!("myelin://{tenant}/git/blob/{}", self.id()))
            .map_err(|_| GitBlobEventKeyError)
    }

    pub fn id(&self) -> String {
        format!(
            "{}:{}:{}",
            self.encoded_repository.as_str(),
            self.encoded_ref.as_str(),
            self.encoded_path.as_str()
        )
    }
}

fn validate_blob_location(ref_name: &str, path: &str) -> Result<(), GitBlobEventKeyError> {
    let is_browsable_ref = {
        let name = RefName::new(ref_name);
        name.validate().is_ok() && RefKind::from_qualified_name(ref_name).is_some()
    };
    if !is_browsable_ref && !is_canonical_object_id(ref_name) {
        return Err(GitBlobEventKeyError);
    }
    if path.is_empty()
        || path.len() > MAX_BLOB_PATH_BYTES
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(GitBlobEventKeyError);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitBlobEventKeyError;

impl std::fmt::Display for GitBlobEventKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid canonical Git blob event key")
    }
}

impl std::error::Error for GitBlobEventKeyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_has_one_encoded_repository_ref_and_path_coordinate() {
        let key = GitBlobEventKey::new("team/api", "refs/heads/main", "src/main.rs").unwrap();
        assert_eq!(key.id(), "team%2Fapi:refs%2Fheads%2Fmain:src%2Fmain%2Ers");
        assert_eq!(
            GitBlobEventKey::parse_id(&key.id()).unwrap(),
            (
                "team/api".into(),
                GitBlobLocation::new("refs/heads/main", "src/main.rs").unwrap()
            )
        );
        assert_eq!(
            key.subject("acme").unwrap().0,
            "myelin://acme/git/blob/team%2Fapi:refs%2Fheads%2Fmain:src%2Fmain%2Ers"
        );
    }

    #[test]
    fn blob_coordinates_reject_aliases_unsafe_paths_and_noncanonical_encoding() {
        for (ref_name, path) in [
            ("main", "src/main.rs"),
            ("refs/notes/build", "src/main.rs"),
            ("refs/heads/main", "../secret"),
            ("refs/heads/main", "/src/main.rs"),
            ("refs/heads/main", "src//main.rs"),
        ] {
            assert!(GitBlobEventKey::new("team/api", ref_name, path).is_err());
        }
        assert!(
            GitBlobEventKey::parse_id("team%2fapi:refs%2Fheads%2Fmain:src%2Fmain%2Ers").is_err()
        );
    }
}
