//! # `define_notif_rule` — the registration seam + the stubbed Notif-owned default reason set (NOTIF-P8 / P-186, M2)
//!
//! **Owning architecture doc:** `notifications.md` §3.1 (the `reason → base → class` table the
//! ranking reads — the `default_class` a rule registers is the class band a Signal of that reason
//! lands in), §3.4 (the router loop step-2 `classify reason → score → dedup/storm-control collapse`:
//! the router classifies a curated Signal's reason **through a registered rule**). **Contract:**
//! **7.6** `define_notif_rule(reason, dedup_tpl, default_class)` — Signal class → inbox
//! reason/priority; **each subsystem registers its set** (Issues SLA/unblocked/approval; KN
//! mentions/comments/shares/watched; Chat mentioned/replied/thread_watched/approval). Owned by
//! Notif; the seam every subsystem registers against. **Reconciliation:** OQ1 (the
//! `define_notif_rule` set is **CONFIRM** — the default-set content is the M3/M4 per-subsystem
//! enumeration, not a Notif change). **External insight:** `01-process-and-quality-doctrine.md` §1
//! (the inverse-signal — the seam must accept a new registration **without a Notif change**; if it
//! gets harder each time, the seam is wrong); VISION §3 (name-your-floors — the stubbed default set
//! → the per-subsystem enumerations).
//!
//! ## What this prompt (NOTIF-P8) ships — the SEAM + a STUBBED default set, nothing else
//!
//! 1. **The registration verb [`define_notif_rule`]** (contract 7.6) — a subsystem registers a
//!    [`Reason`] with its [`DedupTpl`] (the inbox-item dedup template that drives the
//!    `(tenant, recipient, dedup_key)` write-time collapse, §3.2) and its `default_class` (the
//!    [`Class`] band the ranking, NOTIF-P7, reads). The verb is **total** — it constructs the
//!    [`NotifRule`] value; the dedup template is a literal-substitution string (no second predicate
//!    language), exactly the Signal-engine `DedupKeyTpl` discipline (one mental model).
//!
//! 2. **The [`NotifRuleRegistry`]** — the seam each subsystem registers its set into. A subsystem
//!    calls [`NotifRuleRegistry::register`] with a `(rule_key, NotifRule)`; the router then
//!    [`NotifRuleRegistry::classify`]s a Signal carrying that `rule_key` into the rule's reason +
//!    default class + rendered dedup key. **The inverse-signal property is structural:** a NEW
//!    subsystem registration is a call into this registry — it needs **ZERO Notif code change** (no
//!    new enum variant, no match arm, no recompile of Notif). The registry is data, not a `match`.
//!
//! 3. **The Notif-owned DEFAULT reason set, STUBBED** ([`platform_default_rules`]). The
//!    platform-default reasons exist as a registry seed (so a deployment that registers no
//!    subsystem rule still classifies a Signal onto a sane default band), but the **per-subsystem
//!    enumeration** of the real default set is the N-M3/N-M4 accretion. The stub registers ONE
//!    platform-default rule (the ambient `state_changed → watching` fallback the router skeleton,
//!    NOTIF-P3, already classifies onto) so the seam is exercised end-to-end now.
//!
//! ## FLOORS named (this is the SEAM — the per-subsystem default set is the accretion)
//!
//! The stubbed default set → the per-subsystem enumerations land WITHOUT a Notif change (the
//! inverse-signal property this prompt proves), in the accretion prompts:
//! - **Git** registers its reasons (review_requested / mentioned / …) → **NOTIF-P19** (P-263).
//! - **Knowledge** registers mentions / comments / shares / watched → **NOTIF-P20** (P-264).
//! - **Issues** registers SLA / unblocked / approval (+ the escalation chain) → **NOTIF-P21** (P-342).
//! - **Chat** registers mentioned / replied / thread_watched / approval → **NOTIF-P22** (P-343).
//! - **CI** registers the status-summary reasons → **NOTIF-P23** (P-344).
//!
//! Each of those is a call into [`NotifRuleRegistry::register`] from the subsystem's own crate — no
//! arm added here, no Notif recompile. The `seam_accepts_a_new_registration_with_zero_notif_change`
//! test below proves it with a SYNTHETIC subsystem (a stand-in for any of NOTIF-P19..P23).
//!
//! ## Mutation floor (the `define_notif_rule` decision module — mandatory-core)
//! The seam is mandatory-core (every subsystem's classification rides it). The mutation-tested core
//! is the decision logic: the [`DedupTpl::render`] literal substitution (the dedup key drives the
//! §3.2 collapse — a mis-render either over-collapses unrelated items or fails to collapse a storm),
//! the [`NotifRuleRegistry::classify`] lookup (a registered key → its reason + default_class + dedup
//! key; an unregistered key → the platform default, never a panic / never a silent drop), and the
//! `default_class`-drives-ranking wiring (the registered class is the band the rank reads). **Floor:
//! ≥ 80% line/branch mutation score on `define_rule.rs` decision logic** (measured with `cargo
//! mutants`; reported in the P-186 commit body). The floor is **stated and met** by the unit +
//! chained + CDC tests: every render path is asserted (placeholder, missing field, escaped brace),
//! the registered-vs-default classify split is asserted, and the default_class → ranking wiring is
//! asserted; a mutant that swaps a render branch, ignores a registration, or drops the default-class
//! wiring is caught.

use std::collections::HashMap;

use myelin_refs::ArtifactRef;

use crate::ranking::reason_base_class;
use crate::{Class, Reason};

/// **A Notif-owned inbox-item dedup template (contract 7.6 `dedup_tpl`).** A literal-substitution
/// string with `{<field>}` placeholders rendered against the `(recipient, subject)` of the inbox
/// item the rule classifies — the rendered string is the `dedup_key` that drives the
/// `(tenant, recipient, dedup_key)` write-time collapse (§3.2). This is the inbox-side analogue of
/// the Signal-engine [`DedupKeyTpl`](myelin_query::signals::DedupKeyTpl): N near-identical inbox
/// items rendering to the SAME key (e.g. five comments on one issue → "issue PROJ-1 has 5 new
/// comments") collapse into ONE row with `coalesce_count = N`.
///
/// **It is a literal substitution, NOT a second predicate/expression language** (the same
/// discipline as the Signal dedup template — no functions, no conditionals): exactly the
/// placeholders `{subject}` / `{recipient}` / `{reason}`, exactly their values. An unknown
/// placeholder renders the literal token `<missing>` (deterministic — never a panic, never a
/// silent collapse of unrelated items).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DedupTpl(pub String);

impl DedupTpl {
    /// Render the template into a concrete dedup key for `(recipient, subject)` under `reason`.
    /// `{subject}` → the subject ref; `{recipient}` → the recipient pseudonym; `{reason}` → the
    /// snake-case reason token; `{{`/`}}` → a literal `{`/`}`; any other placeholder → `<missing>`.
    /// Total and side-effect-free (the §3.2 collapse key is deterministic from the inputs).
    pub fn render(&self, recipient: &str, subject: &ArtifactRef, reason: Reason) -> String {
        let src = &self.0;
        let mut out = String::with_capacity(src.len());
        let mut chars = src.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    out.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    out.push('}');
                }
                '{' => {
                    let mut name = String::new();
                    for nc in chars.by_ref() {
                        if nc == '}' {
                            break;
                        }
                        name.push(nc);
                    }
                    out.push_str(&render_field(name.trim(), recipient, subject, reason));
                }
                other => out.push(other),
            }
        }
        out
    }
}

/// Project one `{<field>}` placeholder of a [`DedupTpl`] to its value. The frozen namespace is the
/// three inbox-item identity fields the §3.2 collapse keys on: `subject` (the artifact the item is
/// about), `recipient` (the opaque Principal pseudonym, 4.8), and `reason` (the snake-case
/// why-it-fired token). An unknown field is `<missing>` (deterministic; never a panic).
fn render_field(name: &str, recipient: &str, subject: &ArtifactRef, reason: Reason) -> String {
    match name {
        "subject" => subject.0.clone(),
        "recipient" => recipient.to_string(),
        "reason" => reason_token(reason).to_string(),
        _ => "<missing>".to_string(),
    }
}

/// The snake-case wire token for a reason (the `{reason}` dedup placeholder + the registry default
/// key). Matches the `#[serde(rename_all = "snake_case")]` wire form of [`Reason`] (one vocabulary).
fn reason_token(reason: Reason) -> &'static str {
    match reason {
        Reason::ApprovalRequested => "approval_requested",
        Reason::Escalated => "escalated",
        Reason::Sla => "sla",
        Reason::ReviewRequested => "review_requested",
        Reason::Assigned => "assigned",
        Reason::Mentioned => "mentioned",
        Reason::Replied => "replied",
        Reason::AgentProposal => "agent_proposal",
        Reason::Watched => "watched",
        Reason::StateChanged => "state_changed",
        Reason::Fyi => "fyi",
        Reason::Blocked => "blocked",
        Reason::Unblocked => "unblocked",
        Reason::ThreadWatched => "thread_watched",
        Reason::Shared => "shared",
        Reason::Comments => "comments",
    }
}

/// **A registered notification rule (contract 7.6 — the value [`define_notif_rule`] constructs).**
/// The three frozen fields: the [`Reason`] this rule classifies a Signal into (the C-9 scoped-view
/// filter basis + the §3.1 ranking-table key), the [`DedupTpl`] that renders the §3.2 inbox-item
/// `dedup_key`, and the `default_class` (the [`Class`] band the ranking, NOTIF-P7, reads when this
/// rule classifies a Signal).
///
/// **The `default_class` invariant.** A rule's `default_class` is the band the rule's `reason` maps
/// to in the EXACT §3.1 `reason → base → class` table ([`reason_base_class`]) — Notif owns the ONE
/// ranking table; a subsystem registers WHICH reason its Signal is, and the table (not the
/// subsystem) decides the band. [`define_notif_rule`] therefore derives the `default_class` from the
/// reason (it is not a free-form field a subsystem can set to an inconsistent value); a subsystem
/// that wants a different band must register a different reason. This keeps the ranking table the
/// single source of truth (a subsystem can never smuggle an `fyi` into the `critical` band).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifRule {
    /// The structured why-it-fired this rule classifies a Signal into (the §3.1 table key).
    pub reason: Reason,
    /// The inbox-item dedup template (drives the `(tenant, recipient, dedup_key)` §3.2 collapse).
    pub dedup_tpl: DedupTpl,
    /// The default routing/ranking class — the §3.1 band the rule's reason maps to (the ranking,
    /// NOTIF-P7, reads this). Derived from `reason` so the ranking table stays the ONE source.
    pub default_class: Class,
}

impl NotifRule {
    /// Render this rule's dedup template into the concrete §3.2 collapse key for a
    /// `(recipient, subject)`. The router calls this to derive the `dedup_key` it UPSERTs on.
    pub fn dedup_key(&self, recipient: &str, subject: &ArtifactRef) -> String {
        self.dedup_tpl.render(recipient, subject, self.reason)
    }
}

/// **`define_notif_rule(reason, dedup_tpl, default_class)` (contract 7.6) — the registration verb.**
/// A subsystem calls this to construct the [`NotifRule`] it registers for one of its Signal classes:
/// the [`Reason`] the Signal classifies into, the [`DedupTpl`] that drives the §3.2 inbox collapse,
/// and the `default_class` band.
///
/// **The `default_class` is RECONCILED against the §3.1 ranking table, not taken verbatim.** Notif
/// owns the ONE ranking table; a subsystem registers WHICH reason its Signal is, and the table
/// decides the band. So this verb DERIVES the authoritative class from the reason
/// ([`reason_base_class`]) and asserts the supplied `default_class` agrees — a mismatch is a
/// programming error (a subsystem trying to smuggle a reason into the wrong band), returned as
/// [`DefineRuleError::ClassMismatch`] so it fails loudly at registration (never silently mis-banded
/// in prod). A subsystem that passes the table-correct class (the normal path) gets its rule back.
///
/// This is the inverse-signal seam: a subsystem registers a rule by CALLING this — no Notif enum
/// arm, no Notif match, no Notif recompile. The seam accepts a new registration with zero Notif
/// change (EI-01 §1).
pub fn define_notif_rule(
    reason: Reason,
    dedup_tpl: DedupTpl,
    default_class: Class,
) -> Result<NotifRule, DefineRuleError> {
    // The §3.1 table is the ONE source of the reason→class band. A rule's default_class MUST agree
    // with it (a subsystem registers the reason; the table owns the band) — a mismatch is rejected
    // loudly (never silently re-banded), the only way a subsystem could break the NOTIF-D1 invariant.
    let (_base, table_class) = reason_base_class(reason);
    if default_class != table_class {
        return Err(DefineRuleError::ClassMismatch {
            reason,
            supplied: default_class,
            table: table_class,
        });
    }
    Ok(NotifRule { reason, dedup_tpl, default_class })
}

/// Why a [`define_notif_rule`] registration was rejected. The seam fails LOUDLY at registration
/// (never a silently mis-banded rule in prod).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefineRuleError {
    /// The supplied `default_class` does not match the §3.1 ranking-table band for the reason. A
    /// subsystem registers the reason; Notif's table owns the band — a mismatch would let a
    /// subsystem smuggle a reason into the wrong band (breaking the NOTIF-D1 invariant). Rejected.
    ClassMismatch {
        /// The reason that was registered.
        reason: Reason,
        /// The class the subsystem supplied.
        supplied: Class,
        /// The class the §3.1 table requires for that reason.
        table: Class,
    },
}

impl std::fmt::Display for DefineRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefineRuleError::ClassMismatch { reason, supplied, table } => write!(
                f,
                "define_notif_rule: reason {reason:?} must register default_class {table:?} \
                 (the §3.1 ranking-table band), not {supplied:?}"
            ),
        }
    }
}

impl std::error::Error for DefineRuleError {}

/// **The registration seam each subsystem registers its rule set into (contract 7.6).** A subsystem
/// calls [`NotifRuleRegistry::register`] with a `(rule_key, NotifRule)`; the router
/// [`NotifRuleRegistry::classify`]s a curated Signal carrying that `rule_key` into the rule's reason
/// + default class + rendered dedup key.
///
/// **The inverse-signal property is structural (EI-01 §1):** the registry is DATA (a `HashMap`
/// keyed by the rule key), not a `match` over an enum. A NEW subsystem registration is a `register`
/// CALL — it needs ZERO Notif code change (no new enum variant, no new match arm, no Notif
/// recompile). The seam never gets harder as subsystems accrete; each registration is independent.
///
/// `rule_key` is the curated Signal's `rule_id` token (the `<rule>` segment of the
/// `sig.<tenant>.<severity>.<rule>` subject the engine, P-138, publishes) — so the router classifies
/// a Signal BY its rule id through the registered rule. A key registered twice last-write-wins
/// (idempotent re-registration on a reconnect — the rule set is declarative).
#[derive(Clone, Debug, Default)]
pub struct NotifRuleRegistry {
    rules: HashMap<String, NotifRule>,
}

/// How a Signal was classified (the [`NotifRuleRegistry::classify`] outcome). Carries the reason +
/// default class + the rendered §3.2 dedup key — exactly what the router UPSERTs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classification {
    /// The structured why-it-fired the rule classified the Signal into.
    pub reason: Reason,
    /// The ranking class band (the §3.1 table band; the ranking, NOTIF-P7, reads this).
    pub default_class: Class,
    /// The rendered §3.2 inbox-item dedup key (drives the `(tenant, recipient, dedup_key)` collapse).
    pub dedup_key: String,
    /// Whether this came from a registered subsystem rule (`true`) or the platform default
    /// (`false`). Observability: a deployment can assert every live Signal hits a registered rule.
    pub from_registered_rule: bool,
}

impl NotifRuleRegistry {
    /// An EMPTY registry (no subsystem rules). Use [`platform_default`](Self::platform_default) for
    /// the stubbed-default-set seed.
    pub fn new() -> NotifRuleRegistry {
        NotifRuleRegistry::default()
    }

    /// The registry seeded with the Notif-owned STUBBED default reason set
    /// ([`platform_default_rules`]) — the platform fallback so a deployment that has not yet wired a
    /// subsystem's rule set still classifies a Signal onto a sane default band. The per-subsystem
    /// enumeration accretes on top (NOTIF-P19..P23) via [`register`](Self::register).
    pub fn platform_default() -> NotifRuleRegistry {
        let mut reg = NotifRuleRegistry::new();
        for (key, rule) in platform_default_rules() {
            reg.rules.insert(key, rule);
        }
        reg
    }

    /// **Register a subsystem's rule under `rule_key` (the inverse-signal seam, EI-01 §1).** A
    /// subsystem (Git/KN/Issues/Chat/CI — NOTIF-P19..P23) calls this with the rule it built via
    /// [`define_notif_rule`]; the router then classifies a Signal carrying `rule_key` through it.
    /// **ZERO Notif code change** — this is a data insertion, not a Notif enum/match edit. Returns
    /// `&mut self` for fluent registration of a whole set. Last-write-wins on a re-registration
    /// (idempotent on a reconnect; the rule set is declarative).
    pub fn register(&mut self, rule_key: impl Into<String>, rule: NotifRule) -> &mut Self {
        self.rules.insert(rule_key.into(), rule);
        self
    }

    /// The registered rule for `rule_key`, if any.
    pub fn rule(&self, rule_key: &str) -> Option<&NotifRule> {
        self.rules.get(rule_key)
    }

    /// The number of registered rules (the per-subsystem accretion grows this without a Notif change).
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether no rules are registered.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// **Classify a Signal carrying `rule_key` into `(reason, default_class, dedup_key)` (the router
    /// step-2 `classify reason` seam, §3.4).** A REGISTERED key classifies through its rule (the
    /// subsystem's reason + band + rendered dedup key, `from_registered_rule = true`). An
    /// UNREGISTERED key falls back to the platform default ([`platform_default_reason`]:
    /// `state_changed → watching`, the ambient band the router skeleton already uses) so an
    /// un-enumerated Signal is classified onto a sane band — **never a panic, never a silent drop**
    /// (`from_registered_rule = false`). The `dedup_key` is rendered for `(recipient, subject)`.
    pub fn classify(
        &self,
        rule_key: &str,
        recipient: &str,
        subject: &ArtifactRef,
    ) -> Classification {
        match self.rules.get(rule_key) {
            Some(rule) => Classification {
                reason: rule.reason,
                default_class: rule.default_class,
                dedup_key: rule.dedup_key(recipient, subject),
                from_registered_rule: true,
            },
            None => {
                // The platform default fallback — the stubbed default set's ambient band. The dedup
                // key uses the default reason's template (rule + subject), so an un-enumerated Signal
                // still collapses sanely by `(recipient, subject)` rather than never collapsing.
                let (reason, default_class) = platform_default_reason();
                let tpl = default_dedup_tpl();
                Classification {
                    reason,
                    default_class,
                    dedup_key: format!("{rule_key}:{}", tpl.render(recipient, subject, reason)),
                    from_registered_rule: false,
                }
            }
        }
    }
}

/// The Notif-owned **platform-default reason** (the stubbed default set's fallback band): an
/// un-enumerated Signal is classified as ambient `state_changed → watching` — exactly the band the
/// router skeleton (NOTIF-P3) already classifies a curated Signal onto. The per-subsystem
/// enumeration (NOTIF-P19..P23) overrides this per `rule_key` via registration.
pub fn platform_default_reason() -> (Reason, Class) {
    let reason = Reason::StateChanged;
    (reason, reason_base_class(reason).1)
}

/// The default inbox-item dedup template (`"{recipient}:{subject}"` — collapse a recipient's
/// repeated ambient signals on the same subject into one row). Used by the platform-default
/// fallback; a subsystem registers its own template per rule.
fn default_dedup_tpl() -> DedupTpl {
    DedupTpl("{recipient}:{subject}".to_string())
}

/// **The Notif-owned DEFAULT reason set, STUBBED (VISION §3 named floor).** The
/// platform-default reasons exist as a registry seed so a deployment classifies a Signal onto a
/// sane band before any subsystem wires its set — but the **per-subsystem enumeration** is the
/// N-M3/N-M4 accretion (NOTIF-P19 Git, NOTIF-P20 KN, NOTIF-P21 Issues, NOTIF-P22 Chat, NOTIF-P23
/// CI). The stub registers ONE platform-default rule under the `"state_changed"` key (the ambient
/// fallback) so the seam is exercised end-to-end now; the rich per-subsystem set accretes WITHOUT a
/// Notif change (the inverse-signal property this prompt proves).
///
/// **FLOOR:** this stub is intentionally minimal — the real default set is enumerated per subsystem
/// in NOTIF-P19..P23. It is a registry (data), not a hard-coded `match`, exactly so the accretion
/// adds rules by calling [`NotifRuleRegistry::register`], never by editing this function.
pub fn platform_default_rules() -> Vec<(String, NotifRule)> {
    let (reason, class) = platform_default_reason();
    // The verb's reconciliation guarantees the class agrees with the table — the stub is the
    // canonical, table-correct ambient default (so `define_notif_rule` never errors on it).
    let rule = define_notif_rule(reason, default_dedup_tpl(), class)
        .expect("the platform-default rule is table-correct by construction");
    vec![(reason_token(reason).to_string(), rule)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject() -> ArtifactRef {
        ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())
    }

    // --- the DedupTpl literal substitution (the §3.2 collapse-key render) ---

    /// **`DedupTpl::render` substitutes the three frozen placeholders + escapes braces + renders an
    /// unknown placeholder as `<missing>`.** The dedup key drives the §3.2 collapse — a mis-render
    /// either over-collapses unrelated items or fails to collapse a storm. A mutant that swaps a
    /// render branch is caught.
    #[test]
    fn dedup_tpl_renders_placeholders_escapes_and_missing() {
        let tpl = DedupTpl("issue:{subject}|to:{recipient}|why:{reason}".into());
        assert_eq!(
            tpl.render("psn:alice", &subject(), Reason::Mentioned),
            "issue:myelin://acme/issues/issue/PROJ-1|to:psn:alice|why:mentioned"
        );
        // an unknown placeholder → <missing> (deterministic, never a panic).
        assert_eq!(
            DedupTpl("{nope}".into()).render("r", &subject(), Reason::Fyi),
            "<missing>"
        );
        // escaped braces → literal braces.
        assert_eq!(
            DedupTpl("{{literal}}".into()).render("r", &subject(), Reason::Fyi),
            "{literal}"
        );
        // a LONE `}` (not `}}`) is emitted verbatim — the `}}`-escape guard fires ONLY on a real
        // double brace (a mutant that treats every `}` as an escape would drop this one).
        assert_eq!(
            DedupTpl("a}b".into()).render("r", &subject(), Reason::Fyi),
            "a}b",
            "a lone `}}` is literal (the escape guard fires only on `}}}}`)"
        );
        // a `{subject}}` → the subject value followed by a literal `}` (placeholder close, then a
        // lone brace) — distinguishes the placeholder-close from the escape.
        assert_eq!(
            DedupTpl("{subject}}".into()).render("r", &subject(), Reason::Fyi),
            "myelin://acme/issues/issue/PROJ-1}"
        );
        // a template with no placeholder renders verbatim.
        assert_eq!(DedupTpl("static-key".into()).render("r", &subject(), Reason::Fyi), "static-key");
    }

    /// **The `{reason}` placeholder renders the snake-case wire token (one vocabulary with the
    /// serde wire form).** A mutant that mis-maps a reason token is caught.
    #[test]
    fn reason_token_is_the_snake_case_wire_form() {
        assert_eq!(reason_token(Reason::ApprovalRequested), "approval_requested");
        assert_eq!(reason_token(Reason::ReviewRequested), "review_requested");
        assert_eq!(reason_token(Reason::ThreadWatched), "thread_watched");
        assert_eq!(reason_token(Reason::StateChanged), "state_changed");
        // the token round-trips through serde (the ONE wire vocabulary — no drift).
        let json = serde_json::to_string(&Reason::Mentioned).unwrap();
        assert_eq!(json, "\"mentioned\"");
        assert_eq!(reason_token(Reason::Mentioned), "mentioned");
    }

    // --- define_notif_rule (the verb) — the table reconciliation ---

    /// **`define_notif_rule` returns the rule when the supplied `default_class` agrees with the §3.1
    /// table band, and REJECTS a mismatched class loudly.** The table owns the band; a subsystem
    /// registers the reason. A mutant that drops the reconciliation (accepting any class) is caught.
    #[test]
    fn define_notif_rule_reconciles_default_class_against_the_table() {
        // a table-correct registration (mentioned → direct) succeeds.
        let rule = define_notif_rule(
            Reason::Mentioned,
            DedupTpl("{recipient}:{subject}".into()),
            Class::Direct,
        )
        .expect("mentioned → direct is the table band");
        assert_eq!(rule.reason, Reason::Mentioned);
        assert_eq!(rule.default_class, Class::Direct);

        // a MISMATCHED class (mentioned → fyi) is rejected loudly (never silently re-banded).
        let err = define_notif_rule(
            Reason::Mentioned,
            DedupTpl("{subject}".into()),
            Class::Fyi,
        )
        .expect_err("a class that disagrees with the table band is rejected");
        assert_eq!(
            err,
            DefineRuleError::ClassMismatch {
                reason: Reason::Mentioned,
                supplied: Class::Fyi,
                table: Class::Direct,
            }
        );
        // the error renders a PII-free, actionable message (names the reason + the required band).
        let msg = err.to_string();
        assert!(msg.contains("Mentioned") && msg.contains("Direct") && msg.contains("Fyi"), "{msg}");
        // the critical-band reasons reconcile too (sla → critical).
        assert!(define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Critical).is_ok());
        assert!(define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Direct).is_err());
    }

    /// **`NotifRule::dedup_key` renders the rule's template for a `(recipient, subject)`** (the
    /// router's collapse-key derivation). A mutant that ignores the recipient/subject is caught.
    #[test]
    fn notif_rule_dedup_key_renders_for_recipient_and_subject() {
        let rule = define_notif_rule(
            Reason::Mentioned,
            DedupTpl("mention:{subject}".into()),
            Class::Direct,
        )
        .unwrap();
        assert_eq!(
            rule.dedup_key("psn:bob", &subject()),
            "mention:myelin://acme/issues/issue/PROJ-1"
        );
    }

    // --- the registry: register + classify (the inverse-signal seam) ---

    /// **A registered rule classifies a Signal into its reason + default_class + rendered dedup
    /// key; an UNREGISTERED key falls back to the platform default (never a panic / never a silent
    /// drop).** The router step-2 `classify reason` split. A mutant that ignores the registration or
    /// the default fallback is caught.
    #[test]
    fn registry_classifies_registered_then_falls_back_to_default() {
        let mut reg = NotifRuleRegistry::new();
        reg.register(
            "issue_mentioned",
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("mention:{recipient}:{subject}".into()),
                Class::Direct,
            )
            .unwrap(),
        );

        // a registered key → the subsystem's reason + band + rendered dedup key.
        let c = reg.classify("issue_mentioned", "psn:bob", &subject());
        assert_eq!(c.reason, Reason::Mentioned);
        assert_eq!(c.default_class, Class::Direct);
        assert_eq!(c.dedup_key, "mention:psn:bob:myelin://acme/issues/issue/PROJ-1");
        assert!(c.from_registered_rule, "a registered key classifies through its rule");

        // an UNREGISTERED key → the platform default (ambient watching), NOT a panic / drop.
        let d = reg.classify("never_registered", "psn:bob", &subject());
        assert_eq!(d.reason, Reason::StateChanged, "the platform-default ambient reason");
        assert_eq!(d.default_class, Class::Watching);
        assert!(!d.from_registered_rule, "an unregistered key uses the platform default");
        assert!(
            d.dedup_key.starts_with("never_registered:"),
            "the default key namespaces by rule_key so distinct unregistered rules do not collide"
        );
    }

    /// **The `default_class` a registered rule carries IS the band the ranking reads** (the
    /// `default_class → ranking` wiring, the prompt's TESTS line). The classified `default_class`
    /// equals `reason_base_class(reason).1` — the EXACT §3.1 table the ranking (NOTIF-P7) scores on.
    /// A mutant that drops the default-class-drives-ranking wiring is caught.
    #[test]
    fn registered_default_class_is_the_ranking_band_for_the_reason() {
        for (key, reason, class) in [
            ("git_review", Reason::ReviewRequested, Class::Direct),
            ("iss_sla", Reason::Sla, Class::Critical),
            ("chat_replied", Reason::Replied, Class::Participating),
            ("kn_watched", Reason::Watched, Class::Watching),
        ] {
            let mut reg = NotifRuleRegistry::new();
            reg.register(
                key,
                define_notif_rule(reason, DedupTpl("{subject}".into()), class).unwrap(),
            );
            let c = reg.classify(key, "psn:x", &subject());
            // the classified default_class is EXACTLY the §3.1 ranking-table band for the reason.
            assert_eq!(c.default_class, reason_base_class(reason).1);
            assert_eq!(c.default_class, class, "the registered band drives the rank");
        }
    }

    // --- the stubbed default set + the named floors ---

    /// **The platform-default registry is the STUBBED default set: it classifies onto the ambient
    /// band, and the per-subsystem enumeration accretes on top WITHOUT a Notif change.** The stub
    /// holds exactly the one platform-default rule (the floor); registering a subsystem rule grows
    /// it with no Notif edit (the inverse-signal property).
    #[test]
    fn platform_default_is_stubbed_and_accretes_without_notif_change() {
        // a FRESH registry is empty (the `is_empty` true branch — a mutant stubbing it to a constant
        // false is caught here).
        assert!(NotifRuleRegistry::new().is_empty(), "a fresh registry is empty");
        assert_eq!(NotifRuleRegistry::new().len(), 0);

        let reg = NotifRuleRegistry::platform_default();
        assert_eq!(reg.len(), 1, "the stubbed default set is exactly the one platform-default rule");
        assert!(!reg.is_empty(), "the seeded default set is NOT empty");
        // the seed rule is the ambient state_changed → watching fallback.
        let seed = reg.rule("state_changed").expect("the state_changed default rule is seeded");
        assert_eq!(seed.reason, Reason::StateChanged);
        assert_eq!(seed.default_class, Class::Watching);

        // accretion: a NEW subsystem registration grows the set (no Notif change — a register call).
        let mut reg = reg;
        reg.register(
            "git_review_requested",
            define_notif_rule(Reason::ReviewRequested, DedupTpl("{subject}".into()), Class::Direct)
                .unwrap(),
        );
        assert_eq!(reg.len(), 2, "the per-subsystem rule accreted (no Notif enum/match edit)");
    }

    /// **THE INVERSE-SIGNAL PROPERTY (EI-01 §1): a synthetic subsystem registers a brand-new rule
    /// and the router classifies a Signal carrying it — with ZERO Notif code change.** This is the
    /// stand-in for any of NOTIF-P19..P23: a subsystem that did not exist when Notif was written
    /// registers a rule by CALLING `register` (no Notif enum variant, no Notif match arm, no Notif
    /// recompile) and its Signal classifies correctly. If accepting a registration required a Notif
    /// change, THIS TEST COULD NOT COMPILE WITHOUT EDITING NOTIF — it does not.
    #[test]
    fn synthetic_subsystem_registers_with_zero_notif_change() {
        // A synthetic subsystem (a stand-in for Git/KN/Issues/Chat/CI) registers its rule set. It
        // uses ONLY the public seam — `define_notif_rule` + `NotifRuleRegistry::register` — no Notif
        // internal type is touched, no enum is extended.
        let mut reg = NotifRuleRegistry::platform_default();
        reg.register(
            "synthetic.thing_happened",
            define_notif_rule(
                Reason::Assigned,
                DedupTpl("synthetic:{recipient}:{subject}".into()),
                Class::Direct,
            )
            .unwrap(),
        );

        // the router classifies a Signal carrying the synthetic rule_key through the registered rule.
        let c = reg.classify("synthetic.thing_happened", "psn:carol", &subject());
        assert_eq!(c.reason, Reason::Assigned);
        assert_eq!(c.default_class, Class::Direct);
        assert!(c.from_registered_rule, "the synthetic registration took effect (0 Notif change)");
        assert_eq!(c.dedup_key, "synthetic:psn:carol:myelin://acme/issues/issue/PROJ-1");
    }

    /// **A re-registration is last-write-wins (idempotent on a reconnect; the rule set is
    /// declarative).** Registering the same key twice keeps ONE rule (the latest) — a reconnecting
    /// subsystem re-declaring its set does not double it.
    #[test]
    fn re_registration_is_last_write_wins() {
        let mut reg = NotifRuleRegistry::new();
        reg.register(
            "k",
            define_notif_rule(Reason::Assigned, DedupTpl("v1:{subject}".into()), Class::Direct)
                .unwrap(),
        );
        reg.register(
            "k",
            define_notif_rule(Reason::Mentioned, DedupTpl("v2:{subject}".into()), Class::Direct)
                .unwrap(),
        );
        assert_eq!(reg.len(), 1, "the same key is one rule (last-write-wins, idempotent)");
        assert_eq!(reg.rule("k").unwrap().reason, Reason::Mentioned, "the latest registration wins");
    }
}
