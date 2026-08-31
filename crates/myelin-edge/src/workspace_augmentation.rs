use std::collections::BTreeSet;

use myelin_agent_service::workspace::validate_workspace_file_path;
use serde::{Deserialize, Serialize};

use crate::error::EdgeError;

pub(crate) const MAX_WORKSPACE_AUGMENTATION_BYTES: usize =
    myelin_identity_service::MAX_AGENT_WORKSPACE_GUIDANCE_BYTES;
pub(crate) const MAX_WORKSPACE_AUGMENTATION_FILES: usize = 8;
const _: () = assert!(
    MAX_WORKSPACE_AUGMENTATION_BYTES == myelin_storage::MAX_AGENT_THREAD_WORKSPACE_GUIDANCE_BYTES
);
const WORKSPACE_AUGMENTATION_PREFIX: &[u8] = b"myelin.workspace-augmentation.v1\0";
const THREAD_INSTRUCTIONS_HEADING: &str = "## Thread-specific instructions";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceFile {
    pub(crate) path: String,
    pub(crate) contents: String,
    pub(crate) strategy: WorkspaceFileStrategy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceFileStrategy {
    Create,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceAugmentation {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agents_md: Option<String>,
    pub(crate) files: Vec<WorkspaceFile>,
}

impl WorkspaceAugmentation {
    pub(crate) fn from_parts(
        agents_md: Option<String>,
        files: Vec<WorkspaceFile>,
    ) -> Result<Option<Self>, EdgeError> {
        if agents_md.is_none() && files.is_empty() {
            return Ok(None);
        }
        let augmentation = Self {
            version: 1,
            agents_md,
            files,
        };
        augmentation.validate()?;
        Ok(Some(augmentation))
    }

    pub(crate) fn merge_for_thread(
        agent_default: Option<&Self>,
        thread_instructions: Option<&str>,
        thread_files: &[WorkspaceFile],
    ) -> Result<Option<Self>, EdgeError> {
        let default_instructions = agent_default.and_then(|default| default.agents_md.as_deref());
        let agents_md = combine_instructions(default_instructions, thread_instructions)?;
        let mut files = agent_default
            .map(|default| default.files.clone())
            .unwrap_or_default();
        files.extend_from_slice(thread_files);
        Self::from_parts(agents_md, files)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, EdgeError> {
        self.validate()?;
        if self.files.is_empty() {
            if let Some(instructions) = self.agents_md.as_deref() {
                return Ok(instructions.as_bytes().to_vec());
            }
        }
        let encoded = serde_json::to_vec(self).map_err(|_| {
            EdgeError::Internal("agent workspace augmentation could not be encoded".into())
        })?;
        let mut payload = Vec::with_capacity(WORKSPACE_AUGMENTATION_PREFIX.len() + encoded.len());
        payload.extend_from_slice(WORKSPACE_AUGMENTATION_PREFIX);
        payload.extend_from_slice(&encoded);
        if payload.len() > MAX_WORKSPACE_AUGMENTATION_BYTES {
            return Err(EdgeError::BadRequest(format!(
                "agent workspace augmentation exceeds the {MAX_WORKSPACE_AUGMENTATION_BYTES}-byte encrypted limit"
            )));
        }
        Ok(payload)
    }

    pub(crate) fn decode(payload: &[u8]) -> Result<Self, EdgeError> {
        if payload.len() > MAX_WORKSPACE_AUGMENTATION_BYTES {
            return Err(EdgeError::Internal(
                "stored agent workspace augmentation exceeds its size limit".into(),
            ));
        }
        let Some(encoded) = payload.strip_prefix(WORKSPACE_AUGMENTATION_PREFIX) else {
            let instructions = std::str::from_utf8(payload).map_err(|_| {
                EdgeError::Internal("stored agent workspace guidance is not UTF-8".into())
            })?;
            return Ok(Self {
                version: 1,
                agents_md: Some(instructions.to_string()),
                files: Vec::new(),
            });
        };
        let augmentation: Self = serde_json::from_slice(encoded).map_err(|_| {
            EdgeError::Internal("stored agent workspace augmentation is invalid".into())
        })?;
        if augmentation.version != 1 {
            return Err(EdgeError::Internal(
                "stored agent workspace augmentation has an unsupported version".into(),
            ));
        }
        augmentation.validate().map_err(|_| {
            EdgeError::Internal("stored agent workspace augmentation is invalid".into())
        })?;
        Ok(augmentation)
    }

    fn validate(&self) -> Result<(), EdgeError> {
        if self.version != 1 {
            return Err(EdgeError::BadRequest(
                "agent workspace augmentation version is unsupported".into(),
            ));
        }
        if let Some(instructions) = self.agents_md.as_deref() {
            validate_workspace_instructions(instructions)?;
        }
        validate_workspace_files(&self.files)?;
        Ok(())
    }
}

pub(crate) fn validate_workspace_instructions(instructions: &str) -> Result<(), EdgeError> {
    if instructions.trim().is_empty()
        || instructions.len() > MAX_WORKSPACE_AUGMENTATION_BYTES
        || instructions.contains('\0')
    {
        return Err(EdgeError::BadRequest(format!(
            "agent workspace instructions must contain 1..={MAX_WORKSPACE_AUGMENTATION_BYTES} UTF-8 bytes and no NUL characters"
        )));
    }
    Ok(())
}

pub(crate) fn validate_workspace_files(files: &[WorkspaceFile]) -> Result<(), EdgeError> {
    if files.len() > MAX_WORKSPACE_AUGMENTATION_FILES {
        return Err(EdgeError::BadRequest(format!(
            "agent workspace supports at most {MAX_WORKSPACE_AUGMENTATION_FILES} declarative files"
        )));
    }
    let mut paths = BTreeSet::new();
    for file in files {
        validate_workspace_file_path(&file.path).map_err(|error| {
            EdgeError::BadRequest(format!("agent workspace file path is invalid: {error}"))
        })?;
        if file.path == "AGENTS.md" || file.path.split('/').next() == Some(".git") {
            return Err(EdgeError::BadRequest(
                "agent workspace files cannot replace AGENTS.md or write inside .git".into(),
            ));
        }
        if file.contents.contains('\0') {
            return Err(EdgeError::BadRequest(
                "agent workspace file contents cannot contain NUL characters".into(),
            ));
        }
        if !paths.insert(file.path.as_str()) {
            return Err(EdgeError::BadRequest(
                "agent workspace file paths must be unique".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn content_digest(payload: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(payload).to_hex())
}

fn combine_instructions(
    agent_default: Option<&str>,
    thread_instructions: Option<&str>,
) -> Result<Option<String>, EdgeError> {
    let combined = match (agent_default, thread_instructions) {
        (None, None) => return Ok(None),
        (Some(default), None) => default.to_string(),
        (None, Some(thread)) => thread.to_string(),
        (Some(default), Some(thread)) => {
            format!(
                "{}\n\n{THREAD_INSTRUCTIONS_HEADING}\n\n{thread}",
                default.trim_end()
            )
        }
    };
    validate_workspace_instructions(&combined)?;
    Ok(Some(combined))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, contents: &str) -> WorkspaceFile {
        WorkspaceFile {
            path: path.into(),
            contents: contents.into(),
            strategy: WorkspaceFileStrategy::Create,
        }
    }

    #[test]
    fn legacy_and_versioned_payloads_round_trip() {
        let legacy = WorkspaceAugmentation::from_parts(Some("Legacy AGENTS.md".into()), vec![])
            .unwrap()
            .unwrap();
        assert_eq!(legacy.encode().unwrap(), b"Legacy AGENTS.md");
        assert_eq!(
            WorkspaceAugmentation::decode(b"Legacy AGENTS.md").unwrap(),
            legacy
        );

        let versioned = WorkspaceAugmentation::from_parts(
            Some("Default workflow".into()),
            vec![file(".myelin/context.md", "Private context")],
        )
        .unwrap()
        .unwrap();
        let payload = versioned.encode().unwrap();
        assert!(payload.starts_with(WORKSPACE_AUGMENTATION_PREFIX));
        assert_eq!(WorkspaceAugmentation::decode(&payload).unwrap(), versioned);
    }

    #[test]
    fn merging_layers_is_bounded_and_rejects_path_collisions() {
        let default = WorkspaceAugmentation::from_parts(
            Some("Default workflow".into()),
            vec![file(".myelin/context.md", "Default context")],
        )
        .unwrap()
        .unwrap();
        let merged = WorkspaceAugmentation::merge_for_thread(
            Some(&default),
            Some("Keep this diagnosis private."),
            &[file(".myelin/task.md", "Task context")],
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            merged.agents_md.as_deref(),
            Some(
                "Default workflow\n\n## Thread-specific instructions\n\nKeep this diagnosis private."
            )
        );
        assert_eq!(merged.files.len(), 2);
        assert!(WorkspaceAugmentation::merge_for_thread(
            Some(&default),
            None,
            &[file(".myelin/context.md", "Thread context")],
        )
        .is_err());
        assert!(WorkspaceAugmentation::from_parts(
            None,
            vec![file(
                "context.md",
                &"x".repeat(MAX_WORKSPACE_AUGMENTATION_BYTES)
            )],
        )
        .unwrap()
        .unwrap()
        .encode()
        .is_err());
    }
}
