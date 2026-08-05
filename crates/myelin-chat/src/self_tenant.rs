use crate::e2e_wedge::ChatE2eArtifact;
#[cfg(any(test, feature = "test-support"))]
use crate::e2e_wedge::run_chat_e2e_wedge;

pub const MYELIN_SELF_TENANT: &str = "myelin";

pub const MYELIN_SELF_REGION: &str = "fr-par";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyelinChannel {
    pub channel_id: &'static str,
    pub title: &'static str,
    pub bodies: Vec<&'static str>,
}

impl MyelinChannel {
    pub fn bodies_round_trip(&self) -> bool {
        self.bodies.iter().all(|md| crate::roundtrips_md(md, &[]))
    }
}

pub fn myelin_chat_channels() -> Vec<MyelinChannel> {
    vec![
        MyelinChannel {
            channel_id: "myelin-incidents",
            title: "#incidents - the platform's own on-call channel",
            bodies: vec![
                "main is **red** - investigating the `deploy` step.",
                "Opened an issue; *triaging* now.",
                "Root cause: a `flaky` retry - fix in review.",
            ],
        },
        MyelinChannel {
            channel_id: "myelin-releases",
            title: "#releases - the platform shipping itself",
            bodies: vec![
                "Cut **M6** - the `self_tenant` done-bar.",
                "Each band closes on a dated green exit-gate scorecard.",
                "~~Blocked~~ on the switch test; now green.",
            ],
        },
        MyelinChannel {
            channel_id: "myelin-design",
            title: "#design - the frozen design-manual conversations",
            bodies: vec![
                "The 13 screens are pinned at `S1`..`S13`.",
                "Every overlay is **glyph + label + colour**, never colour alone.",
                "The composer is *bottom-pinned*; pickers `flip` above when there's no room.",
            ],
        },
    ]
}

#[derive(Clone, Debug)]
#[must_use = "the self_tenant artifact must be checked - an unread RED face silently claims a green Chat \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct ChatSelfTenantArtifact {
    pub date: String,
    pub channels_round_tripped: usize,
    pub channels_total: usize,
    pub e2e_faces: Vec<ChatE2eArtifact>,
}

impl ChatSelfTenantArtifact {
    pub fn is_green(&self) -> bool {
        self.channels_total > 0
            && self.channels_round_tripped == self.channels_total
            && !self.e2e_faces.is_empty()
            && self.e2e_faces.iter().all(|f| f.is_green())
            && self.total_leaks() == 0
    }

    pub fn total_leaks(&self) -> u64 {
        self.e2e_faces.iter().map(|f| f.leaks).sum()
    }

    pub fn summary(&self) -> String {
        format!(
            "P-521 CHAT SELF_TENANT {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             own-channels-round-trip={}/{} e2e-faces={} total-leaks={} verdict={}",
            self.date,
            self.channels_round_tripped,
            self.channels_total,
            self.e2e_faces.iter().filter(|f| f.is_green()).count(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_chat_over_myelins_own_work(date: &str) -> ChatSelfTenantArtifact {
    let channels = myelin_chat_channels();
    let channels_total = channels.len();
    let channels_round_tripped = channels.iter().filter(|c| c.bodies_round_trip()).count();
    let e2e_faces = run_chat_e2e_wedge();
    ChatSelfTenantArtifact {
        date: date.to_string(),
        channels_round_tripped,
        channels_total,
        e2e_faces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn chat_is_green_on_myelins_own_work() {
        let artifact = run_chat_over_myelins_own_work(RUN_DATE);
        assert!(
            artifact.is_green(),
            "chat must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.channels_round_tripped,
            artifact.channels_total,
            "every channel's bodies round-trip: {}",
            artifact.summary()
        );
        assert_eq!(artifact.total_leaks(), 0, "0 leak: {}", artifact.summary());
        assert!(!artifact.e2e_faces.is_empty(), "the E2E faces are driven");
        for f in &artifact.e2e_faces {
            assert!(f.is_green(), "E2E face {} green", f.scenario);
        }
    }

    #[test]
    fn every_channel_body_round_trips_through_the_one_render_path() {
        for channel in myelin_chat_channels() {
            assert!(
                channel.bodies_round_trip(),
                "{} bodies round-trip through the ONE render path",
                channel.channel_id
            );
            assert!(
                !channel.bodies.is_empty(),
                "{} seeds bodies",
                channel.channel_id
            );
        }
    }

    #[test]
    fn the_self_tenant_summary_is_dated_and_self_tenant_framed() {
        let s = run_chat_over_myelins_own_work(RUN_DATE).summary();
        assert!(s.contains("P-521 CHAT SELF_TENANT 2026-06-26"), "dated: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
    }

    #[test]
    fn a_non_round_tripping_channel_reds_the_self_tenant() {
        let mut artifact = run_chat_over_myelins_own_work(RUN_DATE);
        artifact.channels_round_tripped = artifact.channels_total - 1;
        assert!(
            !artifact.is_green(),
            "a non-round-tripping channel reds the self_tenant: {}",
            artifact.summary()
        );
    }
}
