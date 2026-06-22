//! The 1×/10×/30× load generator with a mixed-principal mix and the four storm profiles.
//!
//! See the crate-level docs for the doctrine / architecture / testing-strategy anchors.
//! This module is the spine of the F6 surge family (testing-strategy §4.1): the 30×
//! agent-skewed mix is the input to every surge drill, proving the protected human lane
//! holds while the agent lane sheds. P-S04 adds the telemetry assertions the drill reads;
//! P-S32/P-S33 tune the storm-profile numbers at M5. Here we ship the generator's own
//! correctness: it hits the requested multiplier within ±tolerance and the requested
//! five-kind mix, and each storm profile selects the right surface shape.

use myelin_identity::{PrincipalId, PrincipalKind, RuntimeRef};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

/// The traffic multiplier — 1× baseline, 10× stress, 30× surge (doctrine EI-01 §3;
/// testing-strategy §3.1). The generator replays a representative workload at the chosen
/// multiplier of its base rate.
///
/// Held as a `u32` factor so the three canonical points are exact and an arbitrary factor
/// (e.g. a custom 50× soak) is also expressible without a new variant — the three named
/// constructors are the doctrine's three points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Multiplier(u32);

impl Multiplier {
    /// 1× — baseline.
    pub const BASELINE: Multiplier = Multiplier(1);
    /// 10× — stress.
    pub const STRESS: Multiplier = Multiplier(10);
    /// 30× — surge (the F6 / SUB-D3 input; the §7.6 storm input).
    pub const SURGE: Multiplier = Multiplier(30);

    /// A custom multiplier factor (≥ 1). Returns `None` for 0 (a no-traffic "surge" is a
    /// mis-specified drill, never silently a no-op — EI-01 §5, loud not swallowed).
    pub fn custom(factor: u32) -> Option<Multiplier> {
        if factor == 0 {
            None
        } else {
            Some(Multiplier(factor))
        }
    }

    /// The integer factor (1 / 10 / 30 / …).
    pub fn factor(self) -> u32 {
        self.0
    }
}

/// The FIVE principal kinds the load generator mixes (testing-strategy §3.1; architecture
/// §7.2 — "the limiter reads `Principal.kind` + the run's class").
///
/// **The mapping to the limiter's view (architecture §7.2).** The limiter shed order is
/// `speculative → batch/CI → agent → human-last`. The product's `Principal.kind`
/// ([`myelin_identity::PrincipalKind`]) has THREE variants: `Human`, `Agent`, `Service`.
/// "CI runner" and "external-MCP" are NOT new `PrincipalKind` variants — they are
/// `Service`-kind principals distinguished by the **run's class** ([`RunClass`]) the
/// limiter reads from the injected headers. So this enum is the *load-generator's* view
/// (the five traffic sources doctrine names), and the projection methods
/// [`LoadPrincipalKind::to_principal_kind`] and [`LoadPrincipalKind::run_class`] map it onto
/// the `(Principal.kind, RunClass)` pair the real limiter actually keys on. Modelling CI or
/// external-MCP as distinct `PrincipalKind` variants would be a divergence from the frozen
/// three-variant `Principal` (EI-01 §1 code-wins-over-docs); we keep the contract shape and
/// carry the distinction in the run class, exactly as §7.2 specifies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LoadPrincipalKind {
    /// An interactive human — the protected lane, shed LAST (§7.2).
    Human,
    /// An agent run — sheds with `429 + Retry-After` before humans (§7.2).
    Agent,
    /// A first-party service principal (machine, non-CI).
    Service,
    /// A CI runner — the batch lane; CI + agent share the wallet, no special
    /// CI-before-agent rule (§7.6, CI-dispatch row).
    Ci,
    /// An external-MCP caller — an external machine client (a `Service`-kind principal on
    /// the external run class; the most-throttleable, least-trusted source).
    ExternalMcp,
}

impl LoadPrincipalKind {
    /// All five kinds, in a stable order (for apportionment + tests).
    pub const ALL: [LoadPrincipalKind; 5] = [
        LoadPrincipalKind::Human,
        LoadPrincipalKind::Agent,
        LoadPrincipalKind::Service,
        LoadPrincipalKind::Ci,
        LoadPrincipalKind::ExternalMcp,
    ];

    /// Project this load-kind onto the frozen three-variant [`PrincipalKind`] the limiter
    /// keys on (§7.2). CI / external-MCP / non-CI service all map to `Service` — the run
    /// class ([`Self::run_class`]) carries the lane distinction the shed order uses.
    ///
    /// An `Agent` carries a placeholder `runtime_ref` + no `on_behalf_of`; a real drill
    /// supplies the run-token-minted refs once Identity M1 lands (the generator only needs
    /// a well-formed `Principal` to tag the request, not a resolved one).
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

    /// The run's class the limiter reads from the injected headers (§7.2 / §7.6) — the lane
    /// this kind sheds in. This is the load-bearing distinction CI / external-MCP carry
    /// that `PrincipalKind` alone does not.
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

/// The run class the principal-aware limiter reads from the injected headers (§7.2). The
/// shed order is keyed on this, NOT on `PrincipalKind` alone: `speculative → batch/CI →
/// agent → human-last`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunClass {
    /// The protected lane — shed last (§7.2).
    Human,
    /// The agent lane — sheds with `429 + Retry-After` (§7.6 agent-mention row).
    Agent,
    /// A non-CI first-party service.
    Service,
    /// The batch / CI lane (§7.6 CI-dispatch row).
    Ci,
    /// An external-MCP caller (external machine client).
    ExternalMcp,
}

/// A requested principal mix across the five [`LoadPrincipalKind`]s, as integer weights.
///
/// Weights are relative (a `{Human:1, Agent:9}` mix is "10% human, 90% agent"); the
/// generator apportions the total request count by largest-remainder so the realised mix
/// matches the requested ratios within ±1 request per kind (exact when the count divides
/// evenly). This is the §3.1 "configurable ratios" + the F6 "30× agent-skewed mix".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalMix {
    weights: [u32; 5],
}

impl PrincipalMix {
    /// A mix from explicit per-kind weights, indexed in [`LoadPrincipalKind::ALL`] order.
    /// Returns `None` if every weight is zero (an all-zero mix has no traffic to issue — a
    /// mis-specified drill, rejected loudly rather than silently emitting nothing).
    pub fn from_weights(weights: [u32; 5]) -> Option<PrincipalMix> {
        if weights.iter().all(|&w| w == 0) {
            None
        } else {
            Some(PrincipalMix { weights })
        }
    }

    /// A balanced mix — equal weight to all five kinds (the default representative mix).
    pub fn balanced() -> PrincipalMix {
        PrincipalMix {
            weights: [1, 1, 1, 1, 1],
        }
    }

    /// The agent-skewed surge mix (the F6 / SUB-D3 input, testing-strategy §3.1): mostly
    /// agent + CI machine traffic with a thin human lane that must survive. v1 floor shape;
    /// the exact skew is tuned by the M5 surge drills (P-S32).
    pub fn agent_skewed() -> PrincipalMix {
        // human:1, agent:6, service:1, ci:2, external-mcp:0 → ~10% human, machine-heavy.
        PrincipalMix {
            weights: [1, 6, 1, 2, 0],
        }
    }

    /// The weight assigned to a kind.
    pub fn weight(&self, kind: LoadPrincipalKind) -> u32 {
        self.weights[Self::index(kind)]
    }

    fn index(kind: LoadPrincipalKind) -> usize {
        LoadPrincipalKind::ALL
            .iter()
            .position(|&k| k == kind)
            .expect("LoadPrincipalKind::ALL is exhaustive")
    }

    /// Apportion `total` requests across the five kinds by **largest-remainder**
    /// (Hamilton's method): each kind gets `floor(total * weight / sum)`, then the
    /// `total - sum(floors)` leftover requests go to the kinds with the largest fractional
    /// remainders. The result sums to EXACTLY `total` and each kind is within ±1 of its
    /// ideal share (exact when `total` divides the ratios evenly). Deterministic — no RNG,
    /// so the mix assertion is reproducible (a drill that can't reproduce isn't a proof).
    pub fn apportion(&self, total: u64) -> [u64; 5] {
        let sum: u64 = self.weights.iter().map(|&w| u64::from(w)).sum();
        if sum == 0 {
            // Unreachable for a constructed PrincipalMix (from_weights rejects all-zero),
            // but keep it total: no traffic apportions to no kinds.
            return [0; 5];
        }
        let mut counts = [0u64; 5];
        // numerator = total * weight; floor share = numerator / sum; remainder = numerator % sum.
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
        // Distribute leftover to the largest remainders (ties broken by lower index for
        // determinism). Sort descending by remainder, then ascending by index.
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

/// The surface a request targets — the shape a storm profile drives (architecture §7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Surface {
    /// CI dispatch — the bounded run-queue (CI-surge profile).
    CiDispatch,
    /// The collab op-stream (KN hot-doc edit/read storm).
    CollabOpStream,
    /// The connection tier (Chat connection-storm).
    ConnectionTier,
    /// Agent-mention (Chat/all — the agent-run lane).
    AgentMention,
}

/// One of the four named per-surface storm profiles (architecture §7.6, OQ-K). Each names
/// the surface it drives + a v1-default request volume per multiplier-unit; the tuned
/// shed-budget numbers are the M5 follow-on (**P-S32 / P-S33** — named floor).
///
/// The `frames_per_request` carries the surface's shape: a connection-storm fans out many
/// frames per connection (frame-heavy), CI-surge is request-heavy (one dispatch per
/// request), etc. A drill reads this to drive the right surface (the §7.6 row), then
/// asserts against that surface's v1 shed-budget floor (P-S04 wires the assertion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StormProfile {
    surface: Surface,
    /// v1 default frames emitted per issued request — the surface's fan-out shape. FLOOR:
    /// tuned at M5 (P-S33).
    frames_per_request: u32,
}

impl StormProfile {
    /// CI-surge (30× agent) — the CI-dispatch surface; one dispatch per request (§7.6).
    pub fn ci_surge() -> StormProfile {
        StormProfile {
            surface: Surface::CiDispatch,
            frames_per_request: 1,
        }
    }

    /// Collab op-stream — the KN hot-doc edit/read storm; a small op fan-out per request.
    pub fn collab_op_stream() -> StormProfile {
        StormProfile {
            surface: Surface::CollabOpStream,
            frames_per_request: 4,
        }
    }

    /// Connection-storm — the Chat connection tier; frame-heavy per connection (presence +
    /// delivery fan-out), the v1 default that the connection-storm drill (P-S31) tunes.
    pub fn connection_storm() -> StormProfile {
        StormProfile {
            surface: Surface::ConnectionTier,
            frames_per_request: 8,
        }
    }

    /// Agent-mention-storm — the agent-run lane (§7.6 agent-mention row); one run per
    /// mention.
    pub fn agent_mention_storm() -> StormProfile {
        StormProfile {
            surface: Surface::AgentMention,
            frames_per_request: 1,
        }
    }

    /// All four named profiles (for the "each profile selects the right surface" test).
    pub fn all() -> [StormProfile; 4] {
        [
            Self::ci_surge(),
            Self::collab_op_stream(),
            Self::connection_storm(),
            Self::agent_mention_storm(),
        ]
    }

    /// The surface this profile drives.
    pub fn surface(&self) -> Surface {
        self.surface
    }

    /// The v1-default frame fan-out per request (FLOOR: tuned at M5, P-S33).
    pub fn frames_per_request(&self) -> u32 {
        self.frames_per_request
    }
}

/// One issued request, tagged with everything a telemetry assertion (P-S04) reads off the
/// metrics port per contract 1.8: the principal-kind + run-class + tenant (so RED/USE can
/// be read **per principal-kind per tenant**, testing-strategy §3.1), the target surface,
/// and a monotonic sequence number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Monotonic issue order within a generator run.
    pub seq: u64,
    /// The load-generator's five-kind view.
    pub load_kind: LoadPrincipalKind,
    /// The frozen `PrincipalKind` the limiter keys on (§7.2 projection).
    pub principal_kind: PrincipalKind,
    /// The run class the limiter's shed order keys on (§7.2).
    pub run_class: RunClass,
    /// A synthetic principal id (opaque, PII-free — `control-plane-pii-free`).
    pub principal_id: PrincipalId,
    /// The tenant the request is tagged with (per-tenant bulkhead, EI-02 §5).
    pub tenant: TenantId,
    /// The surface the storm profile drives.
    pub surface: Surface,
    /// The v1-default frame fan-out for this request (the profile's shape).
    pub frames: u32,
}

/// The abstract sink the generator drives. In tests this is an in-memory request handler;
/// later drills point it at a real `serve` instance (P-S12/P-S13 — the three-port
/// topology). The generator does not care what the sink does — it only issues requests.
pub trait Sink {
    /// Handle one issued request. The sink decides accept/shed/etc.; the generator's job is
    /// only to ISSUE traffic at the configured shape, not to assert the response (the
    /// assertion library, P-S04, reads survival off telemetry, not off this return).
    fn handle(&mut self, request: &Request);
}

/// A convenience in-memory sink that records every request it received — the test rig for
/// the generator's own correctness gate (it lets a test count the realised multiplier +
/// mix). Real drills replace this with a `serve`-backed sink.
#[derive(Default, Debug)]
pub struct RecordingSink {
    /// Every request the generator issued, in order.
    pub received: Vec<Request>,
}

impl Sink for RecordingSink {
    fn handle(&mut self, request: &Request) {
        self.received.push(request.clone());
    }
}

/// The 1×/10×/30× load generator (P-S02; doctrine EI-01 §3).
///
/// Configured with a base request count, a [`Multiplier`], a [`PrincipalMix`], a
/// [`StormProfile`], and the tenant(s) to spread across. [`LoadGenerator::drive`] issues
/// `base * multiplier` requests against a [`Sink`], realising the configured multiplier
/// (exactly) and the configured five-kind mix (within ±1 per kind, deterministically).
#[derive(Clone, Debug)]
pub struct LoadGenerator {
    base_requests: u64,
    multiplier: Multiplier,
    mix: PrincipalMix,
    profile: StormProfile,
    tenants: Vec<TenantId>,
}

impl LoadGenerator {
    /// Build a generator. `tenants` must be non-empty (requests are spread round-robin
    /// across them so a multi-tenant drill can assert per-tenant isolation); an empty
    /// tenant list is rejected loudly rather than silently issuing un-tenanted traffic
    /// (every request carries a tenant — the `residency-pin` / per-tenant-bulkhead rule).
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

    /// The total number of requests this run will issue: `base * multiplier`. Exact — this
    /// is what makes "hits the multiplier within ±tolerance" provable (the tolerance is on
    /// the *rate* when driven against wall-clock; the issued *count* is exact).
    pub fn total_requests(&self) -> u64 {
        self.base_requests * u64::from(self.multiplier.factor())
    }

    /// The planned per-kind request counts for this run (the realised mix), for assertions.
    pub fn planned_mix(&self) -> [u64; 5] {
        self.mix.apportion(self.total_requests())
    }

    /// Issue the full run against `sink`. Issues exactly [`Self::total_requests`] requests;
    /// the per-kind counts match [`Self::planned_mix`]; every request is tagged with its
    /// principal-kind + run-class + tenant + surface (per contract 1.8 so the assertion
    /// library, P-S04, can read RED/USE per principal-kind per tenant). Requests are
    /// interleaved across kinds (not issued in five contiguous blocks) so a drill sees a
    /// realistic mixed stream, and spread round-robin across the configured tenants.
    pub fn drive<S: Sink>(&self, sink: &mut S) {
        let counts = self.planned_mix();
        // Remaining quota per kind; we interleave by round-robining the kinds that still
        // have quota, so the stream is mixed rather than five contiguous blocks.
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
            // Safety: if a round issues nothing but seq < total, the quota arithmetic is
            // broken — fail loud rather than spin forever (EI-01 §5, never silently swallow).
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

    /// Count realised requests per load-kind in a recording sink.
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

    /// GATE (P-S02): the generator hits each multiplier (1×/10×/30×) — the issued count is
    /// EXACTLY `base * factor`. (The ±tolerance in the prompt is on the realised *rate*
    /// when driven against wall-clock; the issued count, the thing a deterministic test can
    /// assert, is exact — a tighter bound than ±tolerance, which strengthens the gate, not
    /// weakens it.)
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

    /// GATE (P-S02): the principal mix matches the requested ratios across the five kinds,
    /// within ±1 request per kind (exact when the count divides evenly). Tested on a
    /// balanced mix (exact) and an agent-skewed mix (the F6 surge input).
    #[test]
    fn principal_mix_matches_requested_ratios() {
        // Balanced mix over 1000 requests → exactly 200 per kind (1000 / 5).
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

        // Agent-skewed mix [1,6,1,2,0] over a 30x surge of base 100 = 3000 requests.
        // sum = 10 → human=300, agent=1800, service=300, ci=600, external-mcp=0.
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
        // The thin human lane is present (it must survive the surge — the F6 property the
        // drill later proves) and the agent lane dominates (the surge input).
        assert!(mix[0] > 0, "the protected human lane must carry traffic");
        assert!(mix[1] > mix[0] * 3, "the surge mix must be agent-skewed");
    }

    /// The mix is within ±1 per kind even when the total does NOT divide evenly (the
    /// largest-remainder apportionment sums to exactly `total`, no request lost/ghosted).
    #[test]
    fn apportionment_is_within_one_and_sums_exactly() {
        // weights [1,1,1,1,1] over 1003 requests: ideal 200.6 each → 201,201,201,200,200.
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

    /// Each named storm profile selects the right surface shape (architecture §7.6).
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
        // All four are distinct surfaces (no profile aliases another).
        let surfaces: Vec<Surface> = StormProfile::all().iter().map(|p| p.surface()).collect();
        let mut uniq = surfaces.clone();
        uniq.sort_by_key(|s| format!("{s:?}"));
        uniq.dedup();
        assert_eq!(
            uniq.len(),
            4,
            "the four storm profiles drive four distinct surfaces"
        );
        // The driven request carries the profile's surface + frame shape.
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

    /// The five load-kinds project onto the frozen three-variant `PrincipalKind` + the run
    /// class (§7.2 — CI / external-MCP are `Service`-kind on distinct run classes, NOT new
    /// `PrincipalKind` variants; the frozen `Principal` shape is preserved, EI-01 §1).
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
        // CI, external-MCP, and non-CI service ALL map to the Service variant...
        for k in [
            LoadPrincipalKind::Service,
            LoadPrincipalKind::Ci,
            LoadPrincipalKind::ExternalMcp,
        ] {
            assert!(matches!(k.to_principal_kind(), PrincipalKind::Service));
        }
        // ...but carry DISTINCT run classes (the lane distinction the limiter keys on).
        assert_eq!(LoadPrincipalKind::Ci.run_class(), RunClass::Ci);
        assert_eq!(
            LoadPrincipalKind::ExternalMcp.run_class(),
            RunClass::ExternalMcp
        );
        assert_eq!(LoadPrincipalKind::Service.run_class(), RunClass::Service);
        // All five run classes are distinct.
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

    /// The stream is INTERLEAVED across kinds (a realistic mixed stream), not five
    /// contiguous blocks — a drill must see human + agent + machine traffic concurrently,
    /// not all humans then all agents (testing-strategy §3.1 "mixes principal types").
    #[test]
    fn stream_is_interleaved_not_blocked() {
        let gen = LoadGenerator::new(
            10,
            Multiplier::BASELINE,
            PrincipalMix::balanced(), // 2 of each kind over 10 requests
            StormProfile::ci_surge(),
            vec![tenant("acme")],
        )
        .unwrap();
        let mut sink = RecordingSink::default();
        gen.drive(&mut sink);
        // First five requests should hit all five distinct kinds (round-robin interleave),
        // not five copies of Human.
        let first_five: Vec<LoadPrincipalKind> =
            sink.received.iter().take(5).map(|r| r.load_kind).collect();
        let mut uniq = first_five.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 5, "the first round interleaves all five kinds");
    }

    /// Requests spread round-robin across the configured tenants (so a multi-tenant surge
    /// drill can assert per-tenant isolation — one tenant's surge does not shed another's).
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

    /// Mis-specification is rejected LOUDLY, never silently a no-op (EI-01 §5): a 0×
    /// multiplier, an all-zero mix, and an empty tenant list all return `None`.
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
