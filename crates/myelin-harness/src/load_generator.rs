use myelin_identity::{PrincipalId, PrincipalKind, RuntimeRef};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Multiplier(u32);

impl Multiplier {
    pub const BASELINE: Multiplier = Multiplier(1);
    pub const STRESS: Multiplier = Multiplier(10);
    pub const SURGE: Multiplier = Multiplier(30);

    pub fn custom(factor: u32) -> Option<Multiplier> {
        if factor == 0 {
            None
        } else {
            Some(Multiplier(factor))
        }
    }

    pub fn factor(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LoadPrincipalKind {
    Human,
    Agent,
    Service,
    Ci,
    ExternalMcp,
}

impl LoadPrincipalKind {
    pub const ALL: [LoadPrincipalKind; 5] = [
        LoadPrincipalKind::Human,
        LoadPrincipalKind::Agent,
        LoadPrincipalKind::Service,
        LoadPrincipalKind::Ci,
        LoadPrincipalKind::ExternalMcp,
    ];

    pub fn to_principal_kind(self) -> PrincipalKind {
        match self {
            LoadPrincipalKind::Human => PrincipalKind::Human,
            LoadPrincipalKind::Agent => PrincipalKind::Agent {
                runtime_ref: RuntimeRef("harness://load-agent".into()),
                on_behalf_of: None,
            },
            LoadPrincipalKind::Service | LoadPrincipalKind::Ci | LoadPrincipalKind::ExternalMcp => {
                PrincipalKind::Service
            }
        }
    }

    pub fn run_class(self) -> RunClass {
        match self {
            LoadPrincipalKind::Human => RunClass::Human,
            LoadPrincipalKind::Agent => RunClass::Agent,
            LoadPrincipalKind::Service => RunClass::Service,
            LoadPrincipalKind::Ci => RunClass::Ci,
            LoadPrincipalKind::ExternalMcp => RunClass::ExternalMcp,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunClass {
    Human,
    Agent,
    Service,
    Ci,
    ExternalMcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalMix {
    weights: [u32; 5],
}

impl PrincipalMix {
    pub fn from_weights(weights: [u32; 5]) -> Option<PrincipalMix> {
        if weights.iter().all(|&w| w == 0) {
            None
        } else {
            Some(PrincipalMix { weights })
        }
    }

    pub fn balanced() -> PrincipalMix {
        PrincipalMix {
            weights: [1, 1, 1, 1, 1],
        }
    }

    pub fn agent_skewed() -> PrincipalMix {
        PrincipalMix {
            weights: [1, 6, 1, 2, 0],
        }
    }

    pub fn weight(&self, kind: LoadPrincipalKind) -> u32 {
        self.weights[Self::index(kind)]
    }

    fn index(kind: LoadPrincipalKind) -> usize {
        LoadPrincipalKind::ALL
            .iter()
            .position(|&k| k == kind)
            .expect("LoadPrincipalKind::ALL is exhaustive")
    }

    pub fn apportion(&self, total: u64) -> [u64; 5] {
        let sum: u64 = self.weights.iter().map(|&w| u64::from(w)).sum();
        if sum == 0 {
            return [0; 5];
        }
        let mut counts = [0u64; 5];
        let mut remainders: [(u64, usize); 5] = [(0, 0); 5];
        let mut assigned: u64 = 0;
        for (i, &w) in self.weights.iter().enumerate() {
            let numerator = total * u64::from(w);
            let floor_share = numerator / sum;
            let remainder = numerator % sum;
            counts[i] = floor_share;
            remainders[i] = (remainder, i);
            assigned += floor_share;
        }
        let mut leftover = total - assigned;
        remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut r = 0usize;
        while leftover > 0 {
            let (_, idx) = remainders[r % 5];
            counts[idx] += 1;
            leftover -= 1;
            r += 1;
        }
        counts
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Surface {
    CiDispatch,
    CollabOpStream,
    ConnectionTier,
    AgentMention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StormProfile {
    surface: Surface,
    frames_per_request: u32,
}

impl StormProfile {
    pub fn ci_surge() -> StormProfile {
        StormProfile {
            surface: Surface::CiDispatch,
            frames_per_request: 1,
        }
    }

    pub fn collab_op_stream() -> StormProfile {
        StormProfile {
            surface: Surface::CollabOpStream,
            frames_per_request: 4,
        }
    }

    pub fn connection_storm() -> StormProfile {
        StormProfile {
            surface: Surface::ConnectionTier,
            frames_per_request: 8,
        }
    }

    pub fn agent_mention_storm() -> StormProfile {
        StormProfile {
            surface: Surface::AgentMention,
            frames_per_request: 1,
        }
    }

    pub fn all() -> [StormProfile; 4] {
        [
            Self::ci_surge(),
            Self::collab_op_stream(),
            Self::connection_storm(),
            Self::agent_mention_storm(),
        ]
    }

    pub fn surface(&self) -> Surface {
        self.surface
    }

    pub fn frames_per_request(&self) -> u32 {
        self.frames_per_request
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub seq: u64,
    pub load_kind: LoadPrincipalKind,
    pub principal_kind: PrincipalKind,
    pub run_class: RunClass,
    pub principal_id: PrincipalId,
    pub tenant: TenantId,
    pub surface: Surface,
    pub frames: u32,
}

pub trait Sink {
    fn handle(&mut self, request: &Request);
}

#[derive(Default, Debug)]
pub struct RecordingSink {
    pub received: Vec<Request>,
}

impl Sink for RecordingSink {
    fn handle(&mut self, request: &Request) {
        self.received.push(request.clone());
    }
}

#[derive(Clone, Debug)]
pub struct LoadGenerator {
    base_requests: u64,
    multiplier: Multiplier,
    mix: PrincipalMix,
    profile: StormProfile,
    tenants: Vec<TenantId>,
}

impl LoadGenerator {
    pub fn new(
        base_requests: u64,
        multiplier: Multiplier,
        mix: PrincipalMix,
        profile: StormProfile,
        tenants: Vec<TenantId>,
    ) -> Option<LoadGenerator> {
        if tenants.is_empty() {
            return None;
        }
        Some(LoadGenerator {
            base_requests,
            multiplier,
            mix,
            profile,
            tenants,
        })
    }

    pub fn total_requests(&self) -> u64 {
        self.base_requests * u64::from(self.multiplier.factor())
    }

    pub fn planned_mix(&self) -> [u64; 5] {
        self.mix.apportion(self.total_requests())
    }

    pub fn drive<S: Sink>(&self, sink: &mut S) {
        let counts = self.planned_mix();
        let mut remaining = counts;
        let mut seq: u64 = 0;
        let mut tenant_rr: usize = 0;
        let total = self.total_requests();
        while seq < total {
            let mut issued_this_round = false;
            for (i, &kind) in LoadPrincipalKind::ALL.iter().enumerate() {
                if remaining[i] == 0 {
                    continue;
                }
                remaining[i] -= 1;
                issued_this_round = true;
                let tenant = self.tenants[tenant_rr % self.tenants.len()].clone();
                tenant_rr += 1;
                let request = Request {
                    seq,
                    load_kind: kind,
                    principal_kind: kind.to_principal_kind(),
                    run_class: kind.run_class(),
                    principal_id: PrincipalId(format!("harness://{kind:?}/{seq}")),
                    tenant,
                    surface: self.profile.surface(),
                    frames: self.profile.frames_per_request(),
                };
                sink.handle(&request);
                seq += 1;
                if seq >= total {
                    break;
                }
            }
            assert!(
                issued_this_round,
                "load-generator quota underflow: {seq}/{total} issued but no kind had quota"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    fn realised_mix(sink: &RecordingSink) -> [u64; 5] {
        let mut counts = [0u64; 5];
        for r in &sink.received {
            let idx = LoadPrincipalKind::ALL
                .iter()
                .position(|&k| k == r.load_kind)
                .unwrap();
            counts[idx] += 1;
        }
        counts
    }

    #[test]
    fn hits_each_multiplier_exactly() {
        let base = 100u64;
        for (m, factor) in [
            (Multiplier::BASELINE, 1u64),
            (Multiplier::STRESS, 10),
            (Multiplier::SURGE, 30),
        ] {
            let gen = LoadGenerator::new(
                base,
                m,
                PrincipalMix::balanced(),
                StormProfile::ci_surge(),
                vec![tenant("acme")],
            )
            .unwrap();
            let mut sink = RecordingSink::default();
            gen.drive(&mut sink);
            assert_eq!(
                sink.received.len() as u64,
                base * factor,
                "multiplier {factor}x must issue base*factor requests"
            );
            assert_eq!(gen.total_requests(), base * factor);
        }
    }

    #[test]
    fn principal_mix_matches_requested_ratios() {
        let gen = LoadGenerator::new(
            1000,
            Multiplier::BASELINE,
            PrincipalMix::balanced(),
            StormProfile::ci_surge(),
            vec![tenant("acme")],
        )
        .unwrap();
        let mut sink = RecordingSink::default();
        gen.drive(&mut sink);
        let mix = realised_mix(&sink);
        assert_eq!(mix, [200, 200, 200, 200, 200], "balanced mix must be exact");

        let gen = LoadGenerator::new(
            100,
            Multiplier::SURGE,
            PrincipalMix::agent_skewed(),
            StormProfile::agent_mention_storm(),
            vec![tenant("acme")],
        )
        .unwrap();
        let mut sink = RecordingSink::default();
        gen.drive(&mut sink);
        let mix = realised_mix(&sink);
        assert_eq!(mix, [300, 1800, 300, 600, 0]);
        assert!(mix[0] > 0, "the protected human lane must carry traffic");
        assert!(mix[1] > mix[0] * 3, "the surge mix must be agent-skewed");
    }

    #[test]
    fn apportionment_is_within_one_and_sums_exactly() {
        let mix = PrincipalMix::balanced();
        let counts = mix.apportion(1003);
        assert_eq!(
            counts.iter().sum::<u64>(),
            1003,
            "no request lost or ghosted"
        );
        let ideal = 1003.0 / 5.0;
        for &c in &counts {
            let diff = (c as f64 - ideal).abs();
            assert!(diff <= 1.0, "each kind within ±1 of its ideal share");
        }
    }

    #[test]
    fn storm_profiles_select_the_right_surface() {
        assert_eq!(StormProfile::ci_surge().surface(), Surface::CiDispatch);
        assert_eq!(
            StormProfile::collab_op_stream().surface(),
            Surface::CollabOpStream
        );
        assert_eq!(
            StormProfile::connection_storm().surface(),
            Surface::ConnectionTier
        );
        assert_eq!(
            StormProfile::agent_mention_storm().surface(),
            Surface::AgentMention
        );
        let surfaces: Vec<Surface> = StormProfile::all().iter().map(|p| p.surface()).collect();
        let mut uniq = surfaces.clone();
        uniq.sort_by_key(|s| format!("{s:?}"));
        uniq.dedup();
        assert_eq!(
            uniq.len(),
            4,
            "the four storm profiles drive four distinct surfaces"
        );
        let gen = LoadGenerator::new(
            10,
            Multiplier::BASELINE,
            PrincipalMix::balanced(),
            StormProfile::connection_storm(),
            vec![tenant("acme")],
        )
        .unwrap();
        let mut sink = RecordingSink::default();
        gen.drive(&mut sink);
        assert!(sink
            .received
            .iter()
            .all(|r| r.surface == Surface::ConnectionTier && r.frames == 8));
    }

    #[test]
    fn load_kinds_project_onto_frozen_principal_kind_and_run_class() {
        assert!(matches!(
            LoadPrincipalKind::Human.to_principal_kind(),
            PrincipalKind::Human
        ));
        assert!(matches!(
            LoadPrincipalKind::Agent.to_principal_kind(),
            PrincipalKind::Agent { .. }
        ));
        for k in [
            LoadPrincipalKind::Service,
            LoadPrincipalKind::Ci,
            LoadPrincipalKind::ExternalMcp,
        ] {
            assert!(matches!(k.to_principal_kind(), PrincipalKind::Service));
        }
        assert_eq!(LoadPrincipalKind::Ci.run_class(), RunClass::Ci);
        assert_eq!(
            LoadPrincipalKind::ExternalMcp.run_class(),
            RunClass::ExternalMcp
        );
        assert_eq!(LoadPrincipalKind::Service.run_class(), RunClass::Service);
        let classes: Vec<RunClass> = LoadPrincipalKind::ALL
            .iter()
            .map(|k| k.run_class())
            .collect();
        for i in 0..classes.len() {
            for j in (i + 1)..classes.len() {
                assert_ne!(
                    classes[i], classes[j],
                    "run classes must be distinct per kind"
                );
            }
        }
    }

    #[test]
    fn stream_is_interleaved_not_blocked() {
        let gen = LoadGenerator::new(
            10,
            Multiplier::BASELINE,
            PrincipalMix::balanced(),
            StormProfile::ci_surge(),
            vec![tenant("acme")],
        )
        .unwrap();
        let mut sink = RecordingSink::default();
        gen.drive(&mut sink);
        let first_five: Vec<LoadPrincipalKind> =
            sink.received.iter().take(5).map(|r| r.load_kind).collect();
        let mut uniq = first_five.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 5, "the first round interleaves all five kinds");
    }

    #[test]
    fn requests_spread_across_tenants() {
        let gen = LoadGenerator::new(
            10,
            Multiplier::BASELINE,
            PrincipalMix::balanced(),
            StormProfile::ci_surge(),
            vec![tenant("acme"), tenant("globex")],
        )
        .unwrap();
        let mut sink = RecordingSink::default();
        gen.drive(&mut sink);
        let acme = sink
            .received
            .iter()
            .filter(|r| r.tenant == tenant("acme"))
            .count();
        let globex = sink
            .received
            .iter()
            .filter(|r| r.tenant == tenant("globex"))
            .count();
        assert_eq!(acme, 5);
        assert_eq!(globex, 5);
    }

    #[test]
    fn misspecification_is_loud_not_silent() {
        assert!(
            Multiplier::custom(0).is_none(),
            "0x is a mis-specified surge"
        );
        assert!(
            Multiplier::custom(50).is_some(),
            "a custom 50x soak is valid"
        );
        assert!(
            PrincipalMix::from_weights([0, 0, 0, 0, 0]).is_none(),
            "an all-zero mix has no traffic"
        );
        assert!(
            LoadGenerator::new(
                100,
                Multiplier::SURGE,
                PrincipalMix::balanced(),
                StormProfile::ci_surge(),
                vec![],
            )
            .is_none(),
            "un-tenanted traffic is rejected (every request carries a tenant)"
        );
    }
}
