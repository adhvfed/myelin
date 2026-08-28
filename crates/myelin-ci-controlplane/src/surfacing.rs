use myelin_refs::{ArtifactRef, Sub};

pub const CI_SUBSYSTEM: &str = "ci";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiRefError {
    InvalidComponent { component: &'static str },
    Parse(myelin_refs::ParseError),
}

impl std::fmt::Display for CiRefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiRefError::InvalidComponent { component } => write!(
                f,
                "CI artifact reference component `{component}` is empty or contains a reserved `/` or `#` delimiter"
            ),
            CiRefError::Parse(error) => write!(f, "invalid CI artifact reference: {error}"),
        }
    }
}

impl std::error::Error for CiRefError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CiRefError::InvalidComponent { .. } => None,
            CiRefError::Parse(error) => Some(error),
        }
    }
}

impl From<myelin_refs::ParseError> for CiRefError {
    fn from(error: myelin_refs::ParseError) -> Self {
        CiRefError::Parse(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiArtifactType {
    Run,
    Deployment,
    Pipeline,
    Runner,
    Artifact,
}

impl CiArtifactType {
    pub const fn token(self) -> &'static str {
        match self {
            CiArtifactType::Run => "run",
            CiArtifactType::Deployment => "deployment",
            CiArtifactType::Pipeline => "pipeline",
            CiArtifactType::Runner => "runner",
            CiArtifactType::Artifact => "artifact",
        }
    }
}

pub fn ci_run_ref(tenant: &str, run_id: &str) -> Result<ArtifactRef, CiRefError> {
    mint_root(tenant, CiArtifactType::Run, run_id)
}

pub fn ci_deployment_ref(tenant: &str, dep_id: &str) -> Result<ArtifactRef, CiRefError> {
    mint_root(tenant, CiArtifactType::Deployment, dep_id)
}

pub fn ci_pipeline_ref(tenant: &str, pipeline_id: &str) -> Result<ArtifactRef, CiRefError> {
    mint_root(tenant, CiArtifactType::Pipeline, pipeline_id)
}

pub fn ci_runner_ref(tenant: &str, runner_id: &str) -> Result<ArtifactRef, CiRefError> {
    mint_root(tenant, CiArtifactType::Runner, runner_id)
}

pub fn ci_artifact_ref(tenant: &str, artifact_id: &str) -> Result<ArtifactRef, CiRefError> {
    mint_root(tenant, CiArtifactType::Artifact, artifact_id)
}

fn mint_root(tenant: &str, ty: CiArtifactType, id: &str) -> Result<ArtifactRef, CiRefError> {
    for (component, value) in [("tenant", tenant), ("id", id)] {
        if value.is_empty() || value.contains(['/', '#']) {
            return Err(CiRefError::InvalidComponent { component });
        }
    }
    myelin_refs::parse(&format!(
        "myelin://{tenant}/{CI_SUBSYSTEM}/{}/{id}",
        ty.token()
    ))
    .map_err(CiRefError::from)
}

pub fn run_step_ref(
    run_ref: &ArtifactRef,
    step: u64,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(run_ref, Sub::Step(step))
}

pub fn run_step_line_ref(
    run_ref: &ArtifactRef,
    start: u64,
    end: u64,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(run_ref, Sub::LineRange { start, end })
}

pub fn commit_check_ref(
    root: &ArtifactRef,
    context: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(root, Sub::Check(context.to_string()))
}

#[cfg(test)]
#[path = "surfacing_tests.rs"]
mod tests;
