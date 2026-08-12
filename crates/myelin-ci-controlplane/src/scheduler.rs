mod queries;

pub use queries::{
    AUTHORIZE_JOB_LAUNCH_QUERY, AUTHORIZE_JOB_LAUNCH_V2_QUERY, CANCEL_SUPERSEDED_QUERY,
    CLAIM_QUERY, COMPLETE_JOB_QUERY, CONSUME_CLAIM_QUERY,
    CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY, CONSUME_PREPARATION_CLAIM_QUERY,
    CONSUME_SECRET_WITHHELD_CLAIM_QUERY, HEARTBEAT_QUERY, INSERT_JOB_QUEUE_QUERY,
    READ_COMPLETION_DISPOSITION_QUERY, REAP_QUERY, RENEW_PREPARATION_LEASE_QUERY,
    REQUEUE_PREPARATION_CLAIM_QUERY, RESET_REQUEUED_PREPARATION_CI_JOB_SURFACE_QUERY,
    VERIFY_JOB_LAUNCH_LIVE_QUERY,
};
pub(crate) use queries::{
    READ_EXHAUSTED_COMPLETION_REPLAY_QUERY, READ_PREPARATION_COMPLETION_REPLAY_QUERY,
    READ_SECRET_WITHHELD_COMPLETION_REPLAY_QUERY,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Interactive,
    Batch,
    Deploy,
}

impl Lane {
    pub fn priority(self) -> i32 {
        match self {
            Lane::Interactive => 2,
            Lane::Batch => 1,
            Lane::Deploy => 0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Interactive => "interactive",
            Lane::Batch => "batch",
            Lane::Deploy => "deploy",
        }
    }

    pub fn from_token(token: &str) -> Option<Lane> {
        match token {
            "interactive" => Some(Lane::Interactive),
            "batch" => Some(Lane::Batch),
            "deploy" => Some(Lane::Deploy),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Inserted,
    DuplicateIdem,
}
