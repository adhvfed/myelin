use myelin_events::consumer::{Consumer, ConsumerName, PrefetchBound, Subscription};
use myelin_events::firehose::FirehoseScope;
use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{DedupLedger, EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_refs::ArtifactRef;

use super::{Card, LadderOutcome, RefsResolvePort, Tombstone, TombstoneReason, UnfurlCache};

pub const DEFAULT_CACHE_TTL_SECONDS: u64 = 60;

pub const UNFURL_INVALIDATION_SUBJECTS: &[&str] =
    &["issue.", "git.", "ci.", "knowledge.", "chat.", "identity."];

pub fn invalidates_card(event_type: &str) -> bool {
    if event_type == CI_CHECK_UPDATED {
        return true;
    }
    match event_type.rsplit_once('.') {
        Some((_, event_name)) => {
            matches!(event_name, "updated" | "erased" | "revoked")
        }
        None => false,
    }
}

pub trait CardUpdatePush {
    fn push_card_update(&self, scope: &FirehoseScope, invalidated: &ArtifactRef) -> u64;
}

pub struct UnfurlInvalidator {
    cache: UnfurlCache,
    subjects: &'static [SubjectPattern],
}

use std::sync::OnceLock;
fn invalidation_subject_patterns() -> &'static [SubjectPattern] {
    static PATTERNS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            UNFURL_INVALIDATION_SUBJECTS
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect()
        })
        .as_slice()
}

impl UnfurlInvalidator {
    pub fn new(cache: UnfurlCache) -> UnfurlInvalidator {
        UnfurlInvalidator {
            cache,
            subjects: invalidation_subject_patterns(),
        }
    }

    pub fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    pub fn should_bust(&self, ev: &EventEnvelope) -> bool {
        invalidates_card(&ev.type_.0)
    }

    pub fn invalidate(&self, ev: &EventEnvelope) -> bool {
        if !self.should_bust(ev) {
            return false;
        }
        let subject_ref = ArtifactRef(ev.subject.0.clone());
        let root = myelin_refs::strip_sub(&subject_ref);
        self.cache.bust(&root)
    }

    pub fn with_push<P: CardUpdatePush>(self, push: P) -> LiveUnfurlInvalidator<P> {
        LiveUnfurlInvalidator { inner: self, push }
    }

    pub fn into_consumer(self, name: &str, dedup: DedupLedger) -> Consumer<UnfurlInvalidator> {
        let subscription = Subscription::bind(
            ConsumerName(name.into()),
            UNFURL_INVALIDATION_SUBJECTS,
            PrefetchBound::DEFAULT,
        )
        .expect("the unfurl-invalidation subjects are a `*`-free whitelist (never over-broad)");
        Consumer::new(self, subscription, dedup)
    }
}

impl EventHandler for UnfurlInvalidator {
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        self.invalidate(ev);
        HandleOutcome::Done
    }
}

pub struct LiveUnfurlInvalidator<P: CardUpdatePush> {
    inner: UnfurlInvalidator,
    push: P,
}

impl<P: CardUpdatePush> LiveUnfurlInvalidator<P> {
    pub fn inner(&self) -> &UnfurlInvalidator {
        &self.inner
    }

    pub fn invalidate_and_push(
        &self,
        ev: &EventEnvelope,
        scope: &FirehoseScope,
    ) -> (bool, Option<u64>) {
        let busted = self.inner.invalidate(ev);
        if !self.inner.should_bust(ev) {
            return (false, None);
        }
        let subject_ref = ArtifactRef(ev.subject.0.clone());
        let root = myelin_refs::strip_sub(&subject_ref);
        let seq = self.push.push_card_update(scope, &root);
        (busted, Some(seq))
    }
}

pub fn erasure_safe_rerender<R: RefsResolvePort>(
    cache: &UnfurlCache,
    resolver: &R,
    tenant: &myelin_tenancy::TenantId,
    region: &myelin_tenancy::Region,
    ref_: &ArtifactRef,
    viewer: &myelin_identity::Principal,
    at: &myelin_identity::Consistency,
) -> Card {
    debug_assert!(
        !cache.contains(ref_),
        "erasure_safe_rerender is called AFTER the *.erased bust - the entry must be gone (no durable \
         snapshot; the cache is the only place rendered content lived, §4.5)"
    );
    let outcome = resolver.resolve(tenant, region, ref_, viewer, at);
    match outcome {
        LadderOutcome::Erased(tombstone) | LadderOutcome::Gone(tombstone) => {
            Card::Tombstone(tombstone)
        }
        LadderOutcome::Live(projection) => {
            cache.put(ref_, projection.clone());
            Card::Live {
                projection,
                moved: false,
                outdated: false,
            }
        }
        LadderOutcome::Moved(projection) => {
            cache.put(ref_, projection.clone());
            Card::Live {
                projection,
                moved: true,
                outdated: false,
            }
        }
        LadderOutcome::Outdated(projection) => {
            cache.put(ref_, projection.clone());
            Card::Live {
                projection,
                moved: false,
                outdated: true,
            }
        }
    }
}

pub mod anchor {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum MessageLifecycle {
        Live,
        Deleted,
        Erased,
    }

    pub fn resolve_message_anchor(
        embed: &ArtifactRef,
        lifecycle: MessageLifecycle,
        projection_title: &str,
    ) -> LadderOutcome {
        let sub_anchor = message_sub_anchor(embed);
        let root = myelin_refs::strip_sub(embed);
        match lifecycle {
            MessageLifecycle::Live => LadderOutcome::Live(super::super::Projection {
                title: projection_title.to_string(),
                state: "live".to_string(),
                icon: "message".to_string(),
                sub_anchor,
            }),
            MessageLifecycle::Deleted => LadderOutcome::Gone(Tombstone {
                root,
                reason: TombstoneReason::Gone,
            }),
            MessageLifecycle::Erased => LadderOutcome::Erased(Tombstone {
                root,
                reason: TombstoneReason::Erased,
            }),
        }
    }

    pub fn message_sub_anchor(embed: &ArtifactRef) -> Option<String> {
        embed.0.split_once('#').map(|(_, sub)| sub.to_string())
    }

    pub fn is_dangle_free(outcome: &LadderOutcome) -> bool {
        match outcome {
            LadderOutcome::Live(p) | LadderOutcome::Moved(p) | LadderOutcome::Outdated(p) => {
                p.sub_anchor.as_deref().map(str::is_empty) != Some(true)
            }
            LadderOutcome::Gone(t) | LadderOutcome::Erased(t) => {
                !t.root.0.is_empty() && !t.root.0.contains('#')
            }
        }
    }
}

#[cfg(test)]
mod tests;
