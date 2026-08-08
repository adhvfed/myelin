pub mod cross_language_shim;
pub mod dependency_break;
pub mod drills;
pub mod load_generator;
pub mod restore;
pub mod telemetry;

pub use cross_language_shim::{
    DivergentTierProbe, Nonnegotiable, ShimConformance, ShimEnforcement,
};
pub use dependency_break::{BreakOutcome, Dependency, DependencyBreaker, Scope};
pub use drills::{DrillContext, DrillRegistry, DrillResult, DrillScenario};
pub use load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, Request, RunClass,
    Sink, StormProfile, Surface,
};
pub use restore::{
    BlobAddr, CrossSeamMismatch, CrossSeamReport, IndexDoc, Offset, OltpRow, RestoreOutcome,
    RestoredSnapshot, RestoredSnapshotBuilder, RtoGrain,
};
pub use telemetry::{
    AssertedSignal, Assertion, Label, Predicate, RejectReason, SignalName, SignalSource,
};

pub fn today_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_phase = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_phase + 2) / 5 + 1) as u32;
    let month = (if month_phase < 10 {
        month_phase + 3
    } else {
        month_phase - 9
    }) as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}
