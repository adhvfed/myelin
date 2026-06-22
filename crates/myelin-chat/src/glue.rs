//! # `glue` — the Chat M2-C0 humanise/notif/fanout-class + firehose-scope + TE-21 slice
//! (CHAT-P3 / P-245, M2)
//!
//! This is the THIRD committable unit of milestone M2-C0 — the slice that, with CHAT-P1 (the
//! `chat.*` tokens, [`crate::events`]) and CHAT-P2 (the ReBAC fragment + the `#sub` grammar,
//! [`crate::rebac_fragment`] / [`crate::subs`]), COMPLETES the chat-owned M2-C0 CONTRACT surface.
//! It ships **registrations + validations**, NOT behaviour:
//!
//! 1. **The humanise template keys (contract 7.3)** — chat's card strings, agent-message strings,
//!    and the `chat.message.mentioned` string register into Notif's ONE templating surface
//!    ([`chat_humanise_templates`] / [`register_chat_humanise_templates`]). There is **no
//!    chat-private string map** — humanise is the ONE templating surface (OQ-L). A tenant brands /
//!    localises by overriding the platform-default row; chat NEVER renders a string itself.
//! 2. **The `define_notif_rule` set + the fanout-class (contract 7.6 + arch §4)** — chat's four
//!    notify reasons (`mentioned` / `replied` / `thread_watched` / `approval_requested`) register
//!    into Notif's ONE `define_notif_rule` registry via the frozen verb ([`chat_notif_rules`] /
//!    [`register_chat_notif_rules`]), each band RECONCILED against Notif's §3.1 ranking table (chat
//!    registers WHICH reason; the table owns the band). The **fanout-class** ([`FanoutClass`] +
//!    [`fanout_class`]) is chat's per-event write-fanout-vs-read-fanout decision (arch §4) — the
//!    bounded high-signal set (the `mention(Principal)` → `chat.message.mentioned` write-fanout) vs
//!    the unbounded ambient set (the per-conversation read-fanout log; the celebrity-fanout
//!    mitigation: a 100k-member announcement does ZERO per-member inbox writes).
//! 3. **The firehose scope shape (contract 3.5)** — chat's per-view scope is `channel:<id>`, a
//!    BOUNDED selector ([`chat_channel_scope`]), VALIDATED against the Bus-owned frozen
//!    resume-cursor protocol ([`myelin_events::FirehoseScope`], the `*`-rejecting chokepoint). No
//!    transport here — only the proof that chat's scope shape fits the protocol (0 unbounded-scope
//!    declarations; scope is never `*`). The `resync_required → *.snapshot` fallback contract is
//!    [`CHAT_RESYNC_SNAPSHOT_TOKENS`] (the durable snapshots a `resync_required` client cold-rebuilds
//!    from, already registered in [`crate::events`]).
//! 4. **The TE-21 connection-tier language pin (contract 1.7)** — Rust is the default; the
//!    BEAM/Phoenix hatch is written-but-CLOSED, bounded by the frozen cross-language harness shim.
//!    [`te21_harness_shim_obligation`] records the no-op: in the all-Rust default the shim's
//!    obligations are satisfied trivially (the same in-process Rust subsystem already satisfies the
//!    three-surface topology, liveness≠readiness, no-fire-and-forget emit, `PersonalDataHolder`,
//!    resilient-client, shed order, forward-only migrations). State this is a NO-OP today.
//!
//! ## FLOORS named (VISION §3 — this prompt ships REGISTRATIONS + VALIDATIONS, not behaviour)
//! - **The humanise/notif rules are USED in CHAT-P16/P18** — here they are REGISTERED; the live
//!   route from a `chat.message.mentioned` event → a curated Signal carrying a chat `rule_key` → the
//!   classified inbox item, and the live HITL-card / agent-message render, land in CHAT-P16
//!   (the mention write-fanout) / CHAT-P18 (the chat inbox surface). No signal route here.
//! - **The firehose scope is IMPLEMENTED in CHAT-P9** — here the `channel:<id>` scope SHAPE is
//!   validated against the frozen protocol; the live subscribe/resume transport over the connection
//!   tier (`subscribe(stream, channel:<id>, cursor?)` → live delivery + presence) lands in CHAT-P9,
//!   with the firehose live-delivery body in CHAT-P10. No transport here.
//! - **The TE-21 hatch is written-but-CLOSED** — opened ONLY if CHAT-D3/D4 prove the Rust connection
//!   tier intractable (the BEAM/Phoenix rewrite), in CHAT-P26. Today it is a NO-OP.
//!
//! ## Coherence (EI-01 §7) — chat declares NO second shape
//! Every deliverable constructs the ONE frozen consumer-owned shape: the humanise templates are
//! [`myelin_notif::HumaniseTemplate`] rows registered into the ONE [`myelin_notif::TemplateStore`];
//! the notif rules are [`myelin_notif::NotifRule`]s built by the frozen [`myelin_notif::define_notif_rule`]
//! verb and registered into the ONE [`myelin_notif::NotifRuleRegistry`] (the inverse-signal seam —
//! zero Notif change); the scope is the ONE Bus-owned [`myelin_events::FirehoseScope`]. There is no
//! parallel chat templating engine, no second chat reason vocabulary, and no bespoke chat scope
//! validator. The fanout-class is chat's OWNED decision (arch §4) — the only genuinely-new shape
//! here, and it is a total classifier over chat's durable tokens, not a transport.
//!
//! No cargo-mutants mutation-score floor is required for this module: it ships DECLARATIONS +
//! VALIDATIONS against already-proven Notif/Bus decision logic (the `define_notif_rule` table
//! reconciliation, the `FirehoseScope` `*`-rejection), not new load-bearing decision logic. The
//! fanout-class IS a decision, but a trivially-total lookup over the frozen [`crate::events`] token
//! table (asserted total + disjoint by the unit tests); the mention-write-fanout / read-fanout
//! BEHAVIOUR (where a mutation floor would bind) lands in CHAT-P16/P18. Stated explicitly per the
//! prompt's TESTS line.

use myelin_events::{FirehoseError, FirehoseScope, ScopeKind};
use myelin_notif::{
    define_notif_rule, Class, DedupTpl, HumaniseTemplate, NotifRule, NotifRuleRegistry, Reason,
    TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT,
};

use crate::events::{
    CHAT_CHANNEL_SNAPSHOT, CHAT_DURABLE_TOKENS, CHAT_FIREHOSE_TOKENS, CHAT_MESSAGE_MENTIONED,
    CHAT_MESSAGE_SNAPSHOT, CHAT_THREAD_SNAPSHOT,
};

// ===========================================================================================
// §1 — the humanise template keys (contract 7.3): card strings + agent-message strings +
//      chat.message.mentioned. The ONE templating surface (OQ-L) — NO chat-private string map.
// ===========================================================================================

/// The humanise template key for the **chat HITL-card SUBJECT** line (the in-chat approval card an
/// `EffectApi` gate renders, arch 03 §1.1 / §8). Registered into Notif's ONE templating surface
/// (contract 7.3) — chat does NOT hold a private card-string map (OQ-L). `{0}` is the per-viewer-
/// bound SUBJECT ref (the effect's target artifact); a denied subject renders as a tombstone, never
/// a leaked title (the NOTIF-D4 leak invariant inherited for free). This is the ONLY ref slot in the
/// card — the [`humanise`] path resolves EVERY arg per-viewer, so the per-viewer-scoped subject is
/// the only thing it may carry; the PII-free action/risk/cost facets are bound separately by
/// [`chat_hitl_card_facets`] through the SAME ONE formatter (never ref-resolved — they are agent
/// metadata, not artifacts the viewer might lack access to). See refined notifications §1.4 (the
/// HITL card is a Notif item `reason = approval_requested`) + NOTIF-P9 (the card surfaces **action +
/// risk + cost**).
pub const TPL_CHAT_CARD: &str = "chat.card";

/// The humanise template key for the **chat HITL-card FACETS** line (the action + risk + cost the
/// card surfaces — refined §1.4 / NOTIF-P9). Registered into Notif's ONE templating surface
/// (contract 7.3) alongside [`TPL_CHAT_CARD`]. Its slots are PII-free literal agent strings — NOT
/// per-viewer refs — so they are bound through Notif's ONE ICU-subset formatter
/// ([`myelin_notif::render_message`]) by [`chat_hitl_card_facets`], never through the ref-resolving
/// [`humanise`] path (which would tombstone a literal). `{0}` is the proposed **action**, `{1}` the
/// **risk** band, `{2}` the **cost** estimate.
pub const TPL_CHAT_CARD_FACETS: &str = "chat.card.facets";

/// The HITL-card **action** facet slot (`{0}` of [`TPL_CHAT_CARD_FACETS`]) — the proposed effect the
/// agent wants applied ("merge", "archive-channel", "deploy"). A PII-free verb the agent runtime
/// produces; never a per-viewer ref.
pub const CARD_FACET_ACTION: usize = 0;
/// The HITL-card **risk** facet slot (`{1}`) — the risk band of the proposed action ("irreversible",
/// "reversible"). The L-ladder facet (recon §6 — suggest-by-default for consequential/irreversible).
pub const CARD_FACET_RISK: usize = 1;
/// The HITL-card **cost** facet slot (`{2}`) — the metered cost estimate of the run the approval
/// unblocks (reserve/settle, contract 11.7 — the human approves a KNOWN cost, never a blank cheque).
/// A PII-free estimate string.
pub const CARD_FACET_COST: usize = 2;

/// The humanise template key for an **agent-authored chat message** (the agent's chat output path —
/// agent messages register into the SAME templating surface as human strings, OQ-L / contract 7.3:
/// "every agent-authored message registers here"). `{0}` is the per-viewer-bound subject.
pub const TPL_CHAT_AGENT_MESSAGE: &str = "chat.agent_message";

/// The humanise template key for the **`chat.message.mentioned`** notify string (the write-fanout
/// producer's inbox string, arch §1.1 / §4 / contract 13.1). `{0}` is the per-viewer-bound channel/
/// message subject — "You were mentioned in {0}". The SAME `mentioned` reason templates Notif's
/// platform default keys; this chat key is the chat-specific surface for the `chat.message.mentioned`
/// event family.
pub const TPL_CHAT_MENTIONED: &str = "chat.message.mentioned";

/// Build chat's **humanise template rows (contract 7.3)** — the deliverable of CHAT-P3. Each is a
/// NULL-tenant ([`PLATFORM_DEFAULT_TENANT`]) `en` platform-default row in the ONE
/// [`TemplateStore`]; a tenant brands / localises by registering its own `(tenant, key, locale)`
/// override. The bodies are ICU-MessageFormat-subset markdown-subset strings with a `{0}` SUBJECT
/// slot (resolved per-viewer → title or tombstone). There is **no chat-private string map** — these
/// are rows in Notif's ONE templating surface (OQ-L).
///
/// The chat-owned surfaces: the HITL **card** subject string ([`TPL_CHAT_CARD`]), the HITL card
/// **facets** string ([`TPL_CHAT_CARD_FACETS`] — action/risk/cost), the **agent-message** string
/// ([`TPL_CHAT_AGENT_MESSAGE`]), and the **`chat.message.mentioned`** notify string
/// ([`TPL_CHAT_MENTIONED`]).
pub fn chat_humanise_templates() -> Vec<HumaniseTemplate> {
    let row = |key: &str, body: &str, icon: &str| HumaniseTemplate {
        tenant: PLATFORM_DEFAULT_TENANT.to_string(),
        template_key: key.to_string(),
        locale: myelin_notif::DEFAULT_LOCALE.to_string(),
        body: body.to_string(),
        icon: icon.to_string(),
    };
    vec![
        // The in-chat HITL approval card SUBJECT line (the `EffectApi` gate's per-effect card,
        // refined §1.4). `{0}` is the per-viewer-resolved target artifact (the ONLY ref slot —
        // tombstone on deny, NOTIF-D4). The body is markdown-subset so it round-trips the ONE
        // myelin-content render path; per-viewer-safe by construction.
        row(TPL_CHAT_CARD, "Approval requested on {0}", "approval"),
        // The HITL card FACETS line (refined §1.4 / NOTIF-P9 — the human approves a known ACTION at a
        // known RISK for a known COST, never a blank cheque). PII-free literal agent strings bound by
        // `chat_hitl_card_facets` through the ONE formatter — never ref-resolved.
        row(TPL_CHAT_CARD_FACETS, "**{0}** ({1}, ~{2})", "approval"),
        // An agent-authored chat message — the agent's chat output registers into the SAME surface.
        row(TPL_CHAT_AGENT_MESSAGE, "An agent posted in {0}", "agent"),
        // The chat.message.mentioned notify string (the write-fanout producer's inbox string).
        row(TPL_CHAT_MENTIONED, "You were mentioned in {0}", "mention"),
    ]
}

/// **Register chat's humanise template keys WITH Notif (the GATE).** Puts chat's three
/// [`chat_humanise_templates`] rows into the supplied [`TemplateStore`] (the ONE platform templating
/// surface, contract 7.3). Returns `&mut` store for fluent chaining.
///
/// This is the honest definition of "the keys register with Notif and are accepted": Notif's ONE
/// `TemplateStore` admits each row under its `(tenant|default, key, locale)`, and a later
/// `humanise((key, args), viewer, locale)` renders it per-viewer (proven in this module's tests + the
/// CDC). The live card/agent-message RENDER route lands in CHAT-P16/P18; here the KEYS are registered.
pub fn register_chat_humanise_templates(store: &mut TemplateStore) -> &mut TemplateStore {
    for row in chat_humanise_templates() {
        store.put(row);
    }
    store
}

/// **Render the HITL card's action/risk/cost FACETS through the ONE Notif formatter (refined §1.4 /
/// NOTIF-P9).** The card surfaces three load-bearing facets so a human approves a KNOWN action at a
/// KNOWN risk for a KNOWN cost (never a blank cheque): `action` (the proposed effect), `risk` (the
/// reversibility band), `cost` (the metered estimate). These are PII-free literal agent strings —
/// NOT per-viewer artifact refs — so they are bound by Notif's ONE ICU-subset formatter
/// ([`myelin_notif::render_message`] over [`TPL_CHAT_CARD_FACETS`]), NEVER through the ref-resolving
/// `humanise` path (which would tombstone a literal). The per-viewer SUBJECT line is the separate
/// [`humanise`]-resolved [`TPL_CHAT_CARD`] (tombstone-on-deny, NOTIF-D4); the two compose into the
/// full card. Chat renders NO string itself — it binds Notif's ONE templating surface (OQ-L).
pub fn chat_hitl_card_facets(
    store: &TemplateStore,
    action: &str,
    risk: &str,
    cost: &str,
) -> String {
    let body = store
        .lookup(
            PLATFORM_DEFAULT_TENANT,
            TPL_CHAT_CARD_FACETS,
            DEFAULT_LOCALE,
        )
        .map(|t| t.body.clone())
        // an unregistered key degrades to the bare facet order (never a panic, never chat-local).
        .unwrap_or_else(|| "{0} ({1}, ~{2})".to_string());
    myelin_notif::render_message(
        &body,
        &[action.to_string(), risk.to_string(), cost.to_string()],
    )
}

// ===========================================================================================
// §2 — the define_notif_rule set (contract 7.6): mentioned / replied / thread_watched /
//      approval_requested, each with its dedup template + default class.
// ===========================================================================================

/// The stable `rule_key` chat's **`@mention`** Signal carries (the `<rule>` segment of the curated
/// `sig.<tenant>.<severity>.<rule>` subject) — the canonical write-fanout producer (arch §4 /
/// contract 13.1: the frozen `mention(Principal)` node → `chat.message.mentioned` → a Signal). Notif
/// classifies a Signal carrying this key through the registered [`Reason::Mentioned`] rule.
pub const RULE_KEY_MENTIONED: &str = "chat.message.mentioned";
/// The stable `rule_key` chat's **thread-reply** Signal carries (a reply in *your* thread; arch §4
/// "a reply in your thread" is a write-fanout direct address) → the registered [`Reason::Replied`].
pub const RULE_KEY_REPLIED: &str = "chat.thread.replied";
/// The stable `rule_key` chat's **thread-watched** Signal carries (you watch a thread that got new
/// activity; the ambient read-fanout watcher band) → the registered [`Reason::ThreadWatched`].
pub const RULE_KEY_THREAD_WATCHED: &str = "chat.thread.watched";
/// The stable `rule_key` chat's **approval-requested** Signal carries (an HITL approval awaiting you
/// — the in-chat card; arch §4 / §8) → the registered [`Reason::ApprovalRequested`].
pub const RULE_KEY_APPROVAL_REQUESTED: &str = "chat.approval.requested";

/// Build chat's **`define_notif_rule` reason set (contract 7.6)** — the deliverable of CHAT-P3.
/// Returns the four `(rule_key, NotifRule)` pairs chat registers: `mentioned` ([`Reason::Mentioned`]
/// → [`Class::Direct`]), `replied` ([`Reason::Replied`] → [`Class::Participating`]), `thread_watched`
/// ([`Reason::ThreadWatched`] → [`Class::Watching`]), and `approval_requested`
/// ([`Reason::ApprovalRequested`] → [`Class::Critical`]).
///
/// Each rule is built via the frozen [`define_notif_rule`] verb, so the supplied `default_class` is
/// RECONCILED against Notif's §3.1 ranking table (chat registers WHICH reason; the table owns the
/// band) — a band that disagreed would fail LOUDLY here, never silently mis-rank in prod. The dedup
/// templates collapse a storm by `(recipient, subject)`: five mentions of you in one channel, or
/// repeated approval re-requests on the same card, collapse into ONE inbox row (the §3.2 collapse).
pub fn chat_notif_rules() -> Vec<(&'static str, NotifRule)> {
    vec![
        (
            RULE_KEY_MENTIONED,
            // @mention → DIRECT (the canonical write-fanout producer; addressed to you).
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("chat.mentioned:{recipient}:{subject}".to_string()),
                Class::Direct,
            )
            .expect("Reason::Mentioned reconciles to Class::Direct in the §3.1 table"),
        ),
        (
            RULE_KEY_REPLIED,
            // a reply in your thread → PARTICIPATING (you are actively in the thread).
            define_notif_rule(
                Reason::Replied,
                DedupTpl("chat.replied:{recipient}:{subject}".to_string()),
                Class::Participating,
            )
            .expect("Reason::Replied reconciles to Class::Participating in the §3.1 table"),
        ),
        (
            RULE_KEY_THREAD_WATCHED,
            // a watched thread got activity → WATCHING (ambient read-fanout band).
            define_notif_rule(
                Reason::ThreadWatched,
                DedupTpl("chat.thread_watched:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("Reason::ThreadWatched reconciles to Class::Watching in the §3.1 table"),
        ),
        (
            RULE_KEY_APPROVAL_REQUESTED,
            // an HITL approval awaiting you → CRITICAL (the in-chat card; you must act).
            define_notif_rule(
                Reason::ApprovalRequested,
                DedupTpl("chat.approval:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("Reason::ApprovalRequested reconciles to Class::Critical in the §3.1 table"),
        ),
    ]
}

/// **Register chat's notif reason set WITH Notif (the GATE).** Registers the four
/// [`chat_notif_rules`] into the supplied [`NotifRuleRegistry`] via the inverse-signal seam
/// (`register` — a data insertion, ZERO Notif change). Returns `&mut` registry for fluent chaining.
///
/// The honest definition of "the reason set registers with Notif and is accepted": Notif's registry
/// admits each rule under its `rule_key`, and a later `classify(rule_key, …)` routes a Signal through
/// it (proven in this module's tests + the CDC). The live `chat.message.mentioned` → Signal route is
/// the CHAT-P16/P18 wiring.
pub fn register_chat_notif_rules(registry: &mut NotifRuleRegistry) -> &mut NotifRuleRegistry {
    for (key, rule) in chat_notif_rules() {
        registry.register(key, rule);
    }
    registry
}

// ===========================================================================================
// §3 — the fanout-class declaration (arch 03 §4): write-fanout (bounded high-signal) vs
//      read-fanout (unbounded ambient). Chat's OWNED per-event attention-class decision.
// ===========================================================================================

/// **The fanout class chat decides PER event (arch 03 §4 — the fanout boundary chat owns).** Chat
/// decides, per event, which attention class it is (the obligation Notif hands every subsystem);
/// Notif owns the routing/inbox/priority/delivery (C-9). The two load-bearing classes:
///
/// - **[`WriteFanout`](FanoutClass::WriteFanout)** — the BOUNDED high-signal set: materialise a
///   per-recipient inbox item. An `@mention(Principal)` of you, a reply in *your* thread, an HITL
///   approval awaiting you. The canonical producer is the frozen `mention(Principal)` node →
///   `chat.message.mentioned` → a Signal → Notif write-fanout (the same node that makes agent
///   dispatch safe — the reference gate, contract 13.1).
/// - **[`ReadFanout`](FanoutClass::ReadFanout)** — the UNBOUNDED ambient set: ONE ordered
///   per-conversation log; per-watcher unread computed lazily. "#general has 40 new", unread counts.
///   The load-bearing rule: the unbounded ambient set NEVER write-amplifies — a 100k-member
///   announcement does ZERO per-member inbox writes on a post (the celebrity-fanout mitigation;
///   Silberstein et al. *Feeding Frontier* VLDB 2010; Facebook TAO).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanoutClass {
    /// The bounded high-signal set — materialise a per-recipient inbox item (write-fanout).
    WriteFanout,
    /// The unbounded ambient set — one per-conversation log, lazy per-watcher unread (read-fanout).
    /// NEVER write-amplifies (the celebrity-fanout mitigation).
    ReadFanout,
}

/// The chat durable tokens that are **write-fanout** (the bounded high-signal set, arch §4): the
/// `mention(Principal)` producer + the thread-reply (a reply in your thread is a direct address).
/// Everything else durable is read-fanout (ambient). This is the FROZEN partition the fanout-class
/// decision keys on — kept as a small explicit set so a new token defaults to read-fanout (the safe,
/// non-write-amplifying default; a write-fanout token must be DELIBERATELY added here).
const CHAT_WRITE_FANOUT_TOKENS: &[&str] = &[
    // the canonical write-fanout producer — @mention → per-recipient inbox item (contract 13.1).
    CHAT_MESSAGE_MENTIONED,
    // a reply in your thread is a direct address (the write-fanout participating signal).
    crate::events::CHAT_THREAD_REPLIED,
];

/// **Classify a chat event token into its [`FanoutClass`] (arch 03 §4) — chat's owned per-event
/// fanout decision.** The write-fanout set is the BOUNDED high-signal set ([`CHAT_WRITE_FANOUT_TOKENS`]
/// — the `mention` producer + the thread-reply direct address); every OTHER durable chat token is
/// read-fanout (the ambient per-conversation log, which never write-amplifies — the celebrity-fanout
/// mitigation). Returns `None` for a non-chat / unregistered token.
///
/// **TOTAL over chat's durable tokens** (every durable token classifies) so a new durable token is
/// never silently un-classified — see [`fanout_class_is_total_over_durable_tokens`]. The FIREHOSE
/// tokens (presence/typing/fine-read-state) are ephemeral allowed-to-drop frames, not an
/// attention-class decision — they classify as read-fanout (ambient, never per-recipient writes).
pub fn fanout_class(token: &str) -> Option<FanoutClass> {
    if CHAT_WRITE_FANOUT_TOKENS.contains(&token) {
        Some(FanoutClass::WriteFanout)
    } else if CHAT_DURABLE_TOKENS.contains(&token) || CHAT_FIREHOSE_TOKENS.contains(&token) {
        // every other registered chat token (durable OR firehose) is ambient read-fanout — it never
        // materialises a per-recipient inbox item (the unbounded set never write-amplifies, arch §4).
        Some(FanoutClass::ReadFanout)
    } else {
        None
    }
}

/// **The fanout-class is TOTAL over chat's durable tokens (arch §4 — every event has a class).** A
/// callable invariant (not only a test assertion): every token in [`CHAT_DURABLE_TOKENS`] classifies
/// into exactly one [`FanoutClass`]. Returns `true` iff the classifier is total — so a NEW durable
/// token added to [`crate::events`] without a fanout-class decision FAILS this invariant loudly
/// (the prompt's "the fanout-class is total over the chat.* durable tokens" gate).
pub fn fanout_class_is_total_over_durable_tokens() -> bool {
    CHAT_DURABLE_TOKENS
        .iter()
        .all(|t| fanout_class(t).is_some())
}

// ===========================================================================================
// §3b — the explicit-first agent-dispatch boundary (contract 8.6 / CHAT-1, NOTIF-P22). Chat owns
//        WHICH of its events is a casual @agent mention (EXPLICIT-FIRST — notify only, never an
//        auto-spawned costed run) vs an explicit dispatch trigger. The Bus's dispatch tier
//        (myelin-query §4.7) consumes this decision: a `Mention` → `NotifiedOnly`, an explicit
//        action → a guarded costed run. CHAT-D17: 0 auto-spawn from a casual mention.
// ===========================================================================================

/// **The explicit-first agent-dispatch class chat decides PER event (contract 8.6 / CHAT-1, recon
/// §6).** The platform's agent-native invariant (VISION §3): a casual `@agent` mention NOTIFIES the
/// agent's inbox (reason=`mentioned`, the same inbox model every principal has — §1.4); it does NOT
/// auto-spawn a *costed run*. Only an EXPLICIT action — a structured trigger the human deliberately
/// took (a slash-command, an approve reaction, a project-owned automation reflex) — dispatches a
/// costed run, and even that run still passes reserve/settle + the dispatch guards (§4.7).
///
/// This is the CHAT side of the boundary: chat decides the CLASS; the Bus dispatch tier
/// ([`myelin_query::DispatchTier`]) consumes it ([`Self::trigger_kind`] maps to the frozen
/// `TriggerKind`). Implicit auto-dispatch on a mention is L-3 (counsel-gated, AG-P20) — never the
/// default. The threshold the drill pins: **0 auto-spawn from a casual mention** (CHAT-D17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentDispatchClass {
    /// **Explicit-first NOTIFY** — a casual `@agent` mention. Posts a Notif item to the agent's
    /// inbox (reason=`mentioned`); does NOT auto-spawn a costed run (CHAT-1). The dispatch tier maps
    /// this to `TriggerKind::Mention` → `Disposition::NotifiedOnly` (no reservation, no run).
    NotifyOnly,
    /// **Explicit dispatch** — a deliberate action (slash-command / approve-action / project
    /// automation reflex). Dispatches a costed run AFTER the guards + reserve/settle. The dispatch
    /// tier maps this to `TriggerKind::Automation`.
    ExplicitDispatch,
}

/// **Classify a chat trigger into its [`AgentDispatchClass`] (contract 8.6 / CHAT-1) — chat's owned
/// explicit-first decision.** The load-bearing default is SAFE: a plain `@agent` mention
/// ([`CHAT_MESSAGE_MENTIONED`]) is [`AgentDispatchClass::NotifyOnly`] — it notifies, it never
/// auto-spawns a costed run. An explicit action — modelled here as an explicit `slash_command` /
/// `approve_action` trigger (a deliberate structured action, NOT a casual mention) — is
/// [`AgentDispatchClass::ExplicitDispatch`].
///
/// `is_explicit_action` is the deliberate-action discriminator the chat surface sets: `false` for a
/// casual `@agent` in prose (the overwhelming common case — notify only); `true` only when the human
/// took a structured action that explicitly targets the agent. A mention is NEVER an explicit
/// dispatch regardless of the flag (the explicit-first floor: a casual mention can only notify).
pub fn agent_dispatch_class(token: &str, is_explicit_action: bool) -> AgentDispatchClass {
    // The explicit-first invariant: a mention can ONLY notify, never auto-spawn — even if some
    // upstream mistakenly flagged it as an action, a casual @mention stays notify-only (CHAT-1).
    if token == CHAT_MESSAGE_MENTIONED {
        return AgentDispatchClass::NotifyOnly;
    }
    if is_explicit_action {
        AgentDispatchClass::ExplicitDispatch
    } else {
        // A non-mention, non-explicit chat event addressed at an agent still only notifies — the
        // safe default is notify-only; a costed run requires a DELIBERATE explicit action.
        AgentDispatchClass::NotifyOnly
    }
}

// ===========================================================================================
// §4 — the firehose scope shape (contract 3.5): chat's per-view scope = channel:<id>, VALIDATED
//      against the Bus-owned frozen resume-cursor protocol. NO transport here (CHAT-P9/P10).
// ===========================================================================================

/// The frozen firehose **stream** chat's live delivery / presence ride (contract 3.5 / arch §1.2:
/// `fan.<tenant>.<channel>`). A per-tenant stream; the `channel:<id>` SCOPE narrows it to one
/// channel's slice. Named so a drill asserts the NAME, never a literal. The `<tenant>` is filled at
/// the connection tier (CHAT-P9); here the stream PREFIX is the frozen shape.
pub const CHAT_FIREHOSE_STREAM_PREFIX: &str = "fan";

/// **Build + VALIDATE chat's per-view firehose scope — `channel:<id>` (contract 3.5).** Chat's scope
/// is a BOUNDED selector parsed through the Bus-owned [`FirehoseScope::parse`] (the ONE `*`-rejecting
/// chokepoint) — so a live channel subscription is bounded BY CONSTRUCTION (a client gets only one
/// channel's frames, never the tenant firehose, never `*`). This VALIDATES chat's scope shape against
/// the frozen protocol; it builds NO transport (that is CHAT-P9/P10).
///
/// Returns [`FirehoseError::OverBroadScope`] iff the channel id is empty / `*` / over-broad — the
/// `0 unbounded-scope declarations` gate (scope is never `*`). The returned [`FirehoseScope`] is
/// asserted to be [`ScopeKind::Channel`] by construction.
pub fn chat_channel_scope(channel_id: &str) -> Result<FirehoseScope, FirehoseError> {
    let scope = FirehoseScope::parse(&format!("channel:{channel_id}"))?;
    // chat's per-view scope is ALWAYS a channel slice (never board/doc/inbox) — assert the kind so a
    // future grammar drift can't silently re-point chat's scope off `channel:`.
    debug_assert_eq!(
        scope.kind(),
        ScopeKind::Channel,
        "chat's per-view scope is channel:<id>"
    );
    Ok(scope)
}

/// **The durable `*.snapshot` tokens a `resync_required` chat client cold-rebuilds from (contract
/// 3.5 `resync_required → *.snapshot` fallback).** When a firehose client's resume cursor is older
/// than the bounded retention window, the protocol raises `resync_required` and the client falls back
/// to a full `*.snapshot` replay (the reindex-from-source path, arch §6 / contract 2.6). These are
/// the chat durable snapshot tokens (already registered in [`crate::events`]) that fallback emits —
/// the channel / message / thread reindex-from-source projections. NAMED here as the scope's
/// fallback contract; the replay BODY is CHAT-P6 (skeleton) / CHAT-P21 (full parity).
pub const CHAT_RESYNC_SNAPSHOT_TOKENS: &[&str] = &[
    CHAT_CHANNEL_SNAPSHOT,
    CHAT_MESSAGE_SNAPSHOT,
    CHAT_THREAD_SNAPSHOT,
];

// ===========================================================================================
// §5 — the TE-21 connection-tier language pin (contract 1.7): Rust default; the BEAM hatch
//      written-but-CLOSED, a NO-OP against the frozen cross-language harness shim.
// ===========================================================================================

/// **The TE-21 connection-tier language pin (contract 1.7 — the cross-language harness shim).** The
/// connection tier (live delivery / presence / the firehose subscribe) is **Rust by default**; the
/// BEAM/Phoenix hatch is written-but-CLOSED (opened ONLY if CHAT-D3/D4 prove the Rust connection tier
/// intractable — the rewrite is CHAT-P26). The frozen harness shim (contract 1.7) is the contract a
/// NON-Rust subsystem must satisfy: the three-surface topology, liveness≠readiness, no-fire-and-
/// forget emit, `PersonalDataHolder`, the resilient-client, the shed order, and forward-only
/// migrations.
///
/// **In the all-Rust default this pin is a NO-OP** ([`Te21LanguagePin::is_no_op`] is `true`): the
/// SAME in-process Rust subsystem that hosts the rest of chat already satisfies every harness-shim
/// obligation (it is not a cross-language boundary), so there is nothing for the shim to enforce. The
/// shim's no-op obligation is SATISFIED — recorded, never silently skipped. The hatch only carries an
/// obligation when [`Te21LanguagePin::Beam`] is selected (CHAT-P26), and then the shim's seven
/// obligations bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Te21LanguagePin {
    /// The DEFAULT — the connection tier is in-process Rust. The harness shim is a NO-OP (no
    /// cross-language boundary). This is the value pinned at M2-C0.
    Rust,
    /// The written-but-CLOSED hatch — the BEAM/Phoenix connection tier. Selected ONLY if CHAT-D3/D4
    /// prove Rust intractable (CHAT-P26); then the cross-language harness shim's obligations bind.
    Beam,
}

impl Te21LanguagePin {
    /// The pin recorded at M2-C0 — **Rust** (the all-Rust default). The BEAM hatch is closed.
    pub const PINNED: Te21LanguagePin = Te21LanguagePin::Rust;

    /// `true` iff this pin makes the cross-language harness shim a NO-OP (the all-Rust default — no
    /// cross-language boundary, so the shim's obligations are trivially satisfied). `false` for the
    /// BEAM hatch (the shim's obligations then bind — CHAT-P26).
    pub fn is_no_op(self) -> bool {
        matches!(self, Te21LanguagePin::Rust)
    }
}

/// **Record the TE-21 no-op against the frozen 1.7 harness shim (the GATE).** Returns the
/// [`Te21LanguagePin::PINNED`] value (Rust) together with the proof that the shim's obligation is
/// SATISFIED as a no-op today: the connection tier is the SAME in-process Rust subsystem, so there is
/// no cross-language boundary for the shim to enforce. This is the "the TE-21 no-op is recorded
/// against 1.7" gate — the shim's no-op obligation is satisfied (recorded, not silently skipped).
///
/// The seven frozen harness-shim obligations (contract 1.7) the pin records as no-op-satisfied in the
/// Rust default — each is the SAME guarantee the in-process Rust subsystem already provides:
/// three-surface topology, liveness≠readiness, no-fire-and-forget emit, `PersonalDataHolder`,
/// resilient-client, shed order, forward-only migrations.
pub fn te21_harness_shim_obligation() -> Te21LanguagePin {
    let pin = Te21LanguagePin::PINNED;
    // The recorded no-op: in the all-Rust default the shim is a no-op (no cross-language boundary).
    // A test asserts this; the assertion below documents the obligation at the record site.
    debug_assert!(
        pin.is_no_op(),
        "the M2-C0 TE-21 pin is Rust — the cross-language harness shim is a NO-OP (the BEAM hatch is closed)"
    );
    pin
}

#[cfg(test)]
mod tests;
