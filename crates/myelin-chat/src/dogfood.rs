//! # `dogfood` — Myelin's team talks in its OWN Chat (CHAT-P32 / P-521, M6)
//!
//! **The Chat M6 dogfood half — THE DONE-BAR (chat roadmap §4 "M6 — Dogfooding: the switch test").**
//! M6 promotes NOTHING and freezes NO new contract — the Chat engine is fixed at M4 and hardened
//! through M5 (the outbox co-commit CHAT-P5, the firehose resume-cursor transport CHAT-P9/P10, the one
//! editor render path over the frozen content subset CHAT-P11, the per-viewer unfurls CHAT-P13/P14, the
//! HITL bridge CHAT-P18, the world-scale surge CHAT-P26, the E2E wedge CHAT-P27, the cross-org bridge
//! CHAT-P30). This module MIGRATES Myelin's OWN team chat onto **Myelin Chat** (the team talks in
//! Myelin's own Chat — the cheapest, most honest load generator is the platform's own development,
//! VISION §5 / EI-01 §4) and reaches the switch-test verdict ([`crate::switch_test`]).
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING surface — EI-01 §7)
//! This is a **caller that drives the already-shipped Chat surface over the Myelin self-tenant** —
//! never a second store / transport / render / E2E. It REUSES:
//! - [`crate::roundtrips_md`] → [`myelin_content::wasm`] — the ONE WASM render path (`render(parse(md))
//!   === md`, contract 13.1, the chat KN-D2 round-trip gate). Every chat-message body the team types
//!   round-trips byte-identically — the team's own messages survive the composer with byte-fidelity.
//! - [`crate::run_chat_e2e_wedge`] — the production-hardened E2E faces (the per-viewer leak-free unfurl
//!   pane E2E-1, the HITL exactly-once flagship E2E-2, the erase-reaches-every-holder DSAR E2E-4),
//!   reframed onto Myelin's OWN channels — never a second wedge.
//!
//! ## Myelin's own team chat as Myelin CHANNELS (the team moves off Slack onto its own platform)
//! [`myelin_chat_channels`] is the set of channels Myelin's team migrates (PII-free — the platform's
//! own engineering channels: `#incidents`, `#releases`, `#design`). Every seed message body is a real
//! markdown-subset body whose every inline run round-trips `render(parse(md)) === md` through the ONE
//! WASM render path — [`run_chat_over_myelins_own_work`] asserts the team's own chat survives the
//! composer with byte-fidelity AND drives the three production E2E faces green, 0 leak.
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/04-views-cli-and-api.md` §1 (the 13 screens +
//! the responsive cases). **Roadmap:** `planning/06-roadmaps/subsystems/chat.md` §4 (the M6 switch
//! test) + §6 (the done-bar — CHAT-D19). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §4 (the switch test — drive the real
//! surface), §1 (record honestly — no claimed-but-unearned green). **VISION §5** (Myelin hosts itself).

use crate::e2e_wedge::{run_chat_e2e_wedge, ChatE2eArtifact};

/// The Myelin self-tenant id (the dogfood drives the Chat surface over the platform's OWN work).
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood Chat surface resolves cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

// ───────────────────────────── Myelin's own team chat lives in Chat ─────────────────────────────

/// One of Myelin's OWN channels the team migrates onto Myelin Chat (PII-free — the platform's own
/// engineering channels, the team moving off Slack onto its own platform, VISION §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyelinChannel {
    /// The channel's opaque id (a stable token; the drills assert against the NAME, never a literal).
    pub channel_id: &'static str,
    /// A one-line human title (what the channel is — the team's own work).
    pub title: &'static str,
    /// The markdown-subset message bodies the channel seeds (each must round-trip the ONE render path).
    pub bodies: Vec<&'static str>,
}

impl MyelinChannel {
    /// `true` iff every seed message body round-trips `render(parse(md)) === md` through the ONE WASM
    /// render path (the channel's messages survive the composer with byte-fidelity — the team's own
    /// chat is not silently rewritten). The node array is empty (these seed bodies are plain prose +
    /// the canonical inline marks; the structured `mention`/`artifact_ref`/`embed` nodes are exercised
    /// in the round-trip corpus of the switch test).
    pub fn bodies_round_trip(&self) -> bool {
        self.bodies.iter().all(|md| crate::roundtrips_md(md, &[]))
    }
}

/// **Myelin's OWN team chat as Myelin CHANNELS (`#incidents` / `#releases` / `#design`).** The team
/// talks in its own Chat (VISION §5). Every message body is a canonical markdown-subset body that
/// round-trips through the ONE WASM render path — the dogfood asserts the team's own chat survives the
/// composer with byte-fidelity. PII-free (opaque ids + the team's own process messages).
pub fn myelin_chat_channels() -> Vec<MyelinChannel> {
    vec![
        MyelinChannel {
            channel_id: "myelin-incidents",
            title: "#incidents — the platform's own on-call channel",
            bodies: vec![
                "main is **red** — investigating the `deploy` step.",
                "Opened an issue; *triaging* now.",
                "Root cause: a `flaky` retry — fix in review.",
            ],
        },
        MyelinChannel {
            channel_id: "myelin-releases",
            title: "#releases — the platform shipping itself",
            bodies: vec![
                "Cut **M6** — the `dogfood` done-bar.",
                "Each band closes on a dated green exit-gate scorecard.",
                "~~Blocked~~ on the switch test; now green.",
            ],
        },
        MyelinChannel {
            channel_id: "myelin-design",
            title: "#design — the frozen design-manual conversations",
            bodies: vec![
                "The 13 screens are pinned at `S1`..`S13`.",
                "Every overlay is **glyph + label + colour**, never colour alone.",
                "The composer is *bottom-pinned*; pickers `flip` above when there's no room.",
            ],
        },
    ]
}

/// **The named green artifact the Chat dogfood run emits.** The production-hardened Chat surface driven
/// over Myelin's OWN work, across the production faces:
/// - **Myelin's own channels** ([`myelin_chat_channels`]) — every seed message body round-trips
///   `render(parse(md)) === md` through the ONE WASM render path (the team's chat survives the composer);
/// - **the E2E wedge faces** ([`run_chat_e2e_wedge`]) — the per-viewer leak-free unfurl pane (E2E-1),
///   the HITL exactly-once flagship (E2E-2), the erase-reaches-every-holder DSAR (E2E-4) — REUSED.
///
/// Chat is GREEN on the platform's own work iff every channel round-trips AND every E2E face green AND
/// 0 leak. A face that did not reach green fails LOUDLY ([`ChatDogfoodArtifact::is_green`] is false) —
/// never a claimed-but-unearned green (EI-01 §1/§3).
#[derive(Clone, Debug)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green Chat \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct ChatDogfoodArtifact {
    /// The date the dogfood run was asserted (every face is dated at this run).
    pub date: String,
    /// How many of Myelin's own channels round-tripped every body through the ONE render path
    /// (must == `channels_total`).
    pub channels_round_tripped: usize,
    /// How many of Myelin's own channels the team migrated.
    pub channels_total: usize,
    /// The three E2E wedge faces (the per-viewer unfurl pane / HITL flagship / DSAR holder) — REUSED.
    pub e2e_faces: Vec<ChatE2eArtifact>,
}

impl ChatDogfoodArtifact {
    /// `true` iff Chat is GREEN on Myelin's own work — every channel's bodies round-trip AND every E2E
    /// face green AND 0 leak. The ONLY way to read the dogfood run (a RED face is never silently a pass).
    pub fn is_green(&self) -> bool {
        self.channels_total > 0
            && self.channels_round_tripped == self.channels_total
            && !self.e2e_faces.is_empty()
            && self.e2e_faces.iter().all(|f| f.is_green())
            && self.total_leaks() == 0
    }

    /// The total leak counter across the E2E faces (the F1 leak spine — must be 0).
    pub fn total_leaks(&self) -> u64 {
        self.e2e_faces.iter().map(|f| f.leaks).sum()
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-521 CHAT DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
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

/// **Drive the production Chat surface over Myelin's OWN work (CHAT-P32).** Round-trips every seed
/// message body of every Myelin channel through the ONE WASM render path (the team's chat survives the
/// composer with byte-fidelity), and drives the three production E2E faces (the per-viewer leak-free
/// unfurl pane + the HITL exactly-once flagship + the erase-reaches-every-holder DSAR — REUSED, never
/// a second wedge). Returns the dated [`ChatDogfoodArtifact`]; its [`is_green`] is the earned verdict
/// (every channel round-trips AND every face green AND 0 leak). Reused, never re-implemented (EI-01 §7).
///
/// [`is_green`]: ChatDogfoodArtifact::is_green
pub fn run_chat_over_myelins_own_work(date: &str) -> ChatDogfoodArtifact {
    let channels = myelin_chat_channels();
    let channels_total = channels.len();
    let channels_round_tripped = channels.iter().filter(|c| c.bodies_round_trip()).count();
    let e2e_faces = run_chat_e2e_wedge();
    ChatDogfoodArtifact {
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

    /// **THE DOGFOOD HEADLINE: Chat is GREEN on Myelin's own work.** Every channel's bodies round-trip
    /// through the ONE render path, every E2E face is green, and 0 leak — the team could talk in its own
    /// Chat (VISION §5).
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

    /// Every seed message body of every Myelin channel round-trips `render(parse(md)) === md` (the
    /// team's own chat survives the composer with byte-fidelity — the round-trip leg is REAL, over the
    /// canonical inline marks the team types).
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

    /// The dogfood summary is DATED + self-tenant-framed + reports the verdict (the artifact body).
    #[test]
    fn the_dogfood_summary_is_dated_and_self_tenant_framed() {
        let s = run_chat_over_myelins_own_work(RUN_DATE).summary();
        assert!(s.contains("P-521 CHAT DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
    }

    /// A non-round-tripping channel body reds the dogfood verdict LOUDLY (a silent rewrite is a wall).
    #[test]
    fn a_non_round_tripping_channel_reds_the_dogfood() {
        let mut artifact = run_chat_over_myelins_own_work(RUN_DATE);
        artifact.channels_round_tripped = artifact.channels_total - 1;
        assert!(
            !artifact.is_green(),
            "a non-round-tripping channel reds the dogfood: {}",
            artifact.summary()
        );
    }
}
