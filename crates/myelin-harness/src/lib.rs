pub mod cross_language_shim;
pub mod dependency_break;
pub mod dogfood;
pub mod drills;
pub mod load_generator;
pub mod make_it_real;
pub mod restore;
pub mod scorecard;
pub mod self_hosting_ci;
pub mod telemetry;

pub use cross_language_shim::{
    DivergentTierProbe, Nonnegotiable, ShimConformance, ShimEnforcement,
};
pub use dependency_break::{BreakOutcome, Dependency, DependencyBreaker, Scope};
pub use dogfood::{
    outbox_relay_stall_repro, proven_substrate_rows, ProvenSubstrateRow, SubstrateIncident,
    SubstrateIncidentLoop, SubstrateTruthUpPass, SubstrateTruthUpRed, SubstrateTruthUpVerdict,
};
pub use drills::{DrillContext, DrillRegistry, DrillResult, DrillScenario};
pub use load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, Request, RunClass,
    Sink, StormProfile, Surface,
};
pub use restore::{
    BlobAddr, CrossSeamMismatch, CrossSeamReport, IndexDoc, Offset, OltpRow, RestoreOutcome,
    RestoredSnapshot, RestoredSnapshotBuilder, RtoGrain,
};
pub use scorecard::{Band, GateRow, RowResult, RowVerdict, Scorecard};
pub use self_hosting_ci::{
    run_graph, run_job_via_cargo, self_hosting_jobs, JobKind, JobResult, JobRunner, JobTool,
    SelfHostJob, SelfHostingRun,
};
pub use telemetry::{
    AssertedSignal, Assertion, Label, Predicate, RejectReason, SignalName, SignalSource,
};
