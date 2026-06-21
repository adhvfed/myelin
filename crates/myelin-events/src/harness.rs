//! # `harness` — the per-subsystem token-list VALIDATION HARNESS (EB-26 / P-246, M3)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §6.1 (the dotted-name grammar —
//! the AUTHORITY), §6.4 (the seed event names), §6.2 (the subsystem/type tokens). **Contract:**
//! `contract-index.md` row **2.9** ("each subsystem completes its list" — the Bus owns the grammar +
//! seed; each subsystem owns + COMPLETES its full list, validated against the one grammar).
//! **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §7 (reconcile cross-component
//! contracts at the plan layer — one grammar, no per-subsystem drift; the X-5 names anchor).
//!
//! ## What this is (the Bus's narrow half of 2.9 — the registration HARNESS)
//! The grammar validator ([`crate::taxonomy::validate`]) is the ONE gate every dotted `type` name is
//! checked by ([`crate::taxonomy`], EB-02). This module is the layer ABOVE it: a per-subsystem
//! **list-registration harness**. Each subsystem (Git/KN in M3; CI/Issues/Chat in M4 — EB-27)
//! REGISTERS its COMPLETE dotted-name list as a [`SubsystemTokenList`] — with its `schema_ver`
//! lineage and a payload-shape descriptor per name — and the harness:
//!
//! - **admits the full list** iff every name is grammar-conformant ([`TokenListHarness::register`]
//!   returns `Ok` only when every token parses the §6.1 grammar), AND
//! - **rejects a malformed ADDITION** — a single ungrammatical name added to a list is rejected
//!   LOUDLY with the specific [`crate::taxonomy::TaxonomyError`] it broke + the subsystem it came
//!   from ([`HarnessError`]), never silently coerced (EI-01 §5), AND
//! - holds the **schema_ver lineage** + the **payload-shape descriptor** for each registered name so
//!   the consumer side (and the upcaster registry, contract 2.8) knows the current shape version a
//!   name's payload carries.
//!
//! The Bus owns the GRAMMAR + the harness; the subsystem owns its LIST. A subsystem's list is its
//! OWN constant (`myelin_git::events::GIT_EVENT_TOKENS`, KN's `KNOWLEDGE_EVENT_TOKENS`, …); it
//! registers that list HERE, against the grammar it does not author. One grammar, no drift.
//!
//! ## Why a harness and not just per-subsystem `register_*` helpers
//! Each subsystem already self-checks its list against the grammar in its own crate (`git`'s
//! `register_git_tokens`, etc.). The harness is the **Bus-side, cross-subsystem** registration the
//! 2.9 "each subsystem completes its list" contract names: ONE place that holds every subsystem's
//! completed list keyed by subsystem, enforces no cross-subsystem name collision, enforces the
//! leading-token-matches-subsystem invariant (a subsystem may only register names under ITS own
//! §6.2 prefix — `git` cannot register a `ci.*` name, the acyclic-producer invariant EI-02 §3), and
//! exposes the schema_ver + payload shape the consumer/upcaster legs read. It is the registry the
//! GIT-D9 / KN-D7 carriage drills assert their producers' names against.

use crate::taxonomy::{self, TaxonomyError};
use std::collections::BTreeMap;

/// A coarse payload-shape descriptor for a registered event name (contract 2.9 — "its payload
/// shapes"). The Bus does NOT own the field-level shape (the producing subsystem does); this is the
/// narrow classification the carriage + the upcaster registry (contract 2.8) reason about:
/// references-not-payloads is the platform invariant, so the Bus records WHETHER a name's payload is
/// a pure reference set or carries (key-wrapped) inline PII, plus the current `schema_ver`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PayloadShape {
    /// References-not-payloads: the payload is `ArtifactRef`s / ids only, never inline PII (the vast
    /// majority — the platform default, EI-04). The Bus may carry it on the durable bus freely.
    ReferencesOnly,
    /// The payload carries inline personal data behind a per-subject key (crypto-shreddable, contract
    /// 2.7) — the holder/erasure path must see it (`contains_personal_data = true` on the envelope).
    InlinePersonalData,
    /// An ephemeral firehose-only payload (presence/typing/live) — NEVER the durable bus (ADR-04.5).
    /// Registered so the harness can assert a firehose-only name never rides the durable carriage.
    EphemeralFirehose,
}

/// One registered event name + its `schema_ver` lineage + its payload-shape descriptor (contract
/// 2.9: "with its schema_ver lineage and payload shapes"). The NAME is validated against the §6.1
/// grammar at registration; `current_schema_ver` is the version a live payload of this name carries
/// today (the upcaster registry, contract 2.8, bridges older versions up to it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredToken {
    /// The dotted `type` name (`git.ref.updated`, `knowledge.page.created`, …) — grammar-validated.
    pub name: String,
    /// The current `schema_ver` a live payload of this name carries (≥ 1). A bump here is a schema
    /// evolution the upcaster registry (2.8) must have a `(name, from)→to` bridge for.
    pub current_schema_ver: u32,
    /// The coarse payload-shape descriptor (references-only / inline-PII / firehose).
    pub shape: PayloadShape,
}

impl RegisteredToken {
    /// A references-only token at `schema_ver = 1` (the platform default — the common case).
    pub fn references_only(name: impl Into<String>) -> RegisteredToken {
        RegisteredToken { name: name.into(), current_schema_ver: 1, shape: PayloadShape::ReferencesOnly }
    }

    /// A token carrying inline personal data (crypto-shreddable behind a per-subject key).
    pub fn inline_personal_data(name: impl Into<String>) -> RegisteredToken {
        RegisteredToken {
            name: name.into(),
            current_schema_ver: 1,
            shape: PayloadShape::InlinePersonalData,
        }
    }

    /// A firehose-only token (ephemeral — never the durable bus).
    pub fn firehose(name: impl Into<String>) -> RegisteredToken {
        RegisteredToken { name: name.into(), current_schema_ver: 1, shape: PayloadShape::EphemeralFirehose }
    }

    /// Set the current schema_ver lineage tip (a name whose payload has evolved past v1).
    pub fn at_schema_ver(mut self, v: u32) -> RegisteredToken {
        self.current_schema_ver = v;
        self
    }
}

/// A subsystem's COMPLETE registered dotted-name list (contract 2.9 — "each subsystem completes its
/// list"). The `subsystem` is the §6.2 canonical leading token (`git`, `knowledge`, …); EVERY name
/// in `tokens` MUST carry that prefix (the acyclic-producer invariant — a subsystem only registers
/// names under its OWN prefix, EI-02 §3). The subsystem OWNS this list (it is its own crate
/// constant); it REGISTERS it here against the grammar the Bus owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubsystemTokenList {
    /// The §6.2 canonical subsystem token this list belongs to (the leading dotted segment of every
    /// name in it).
    pub subsystem: String,
    /// The complete set of registered tokens (name + schema_ver + payload shape), in registry order.
    pub tokens: Vec<RegisteredToken>,
}

impl SubsystemTokenList {
    /// Build a list for `subsystem` from a slice of `(name, schema_ver, shape)`-built
    /// [`RegisteredToken`]s. The list is NOT validated here — [`TokenListHarness::register`] is the
    /// gate (so a malformed list is rejected at registration, with the offending token named).
    pub fn new(subsystem: impl Into<String>, tokens: Vec<RegisteredToken>) -> SubsystemTokenList {
        SubsystemTokenList { subsystem: subsystem.into(), tokens }
    }

    /// Build a references-only list for `subsystem` from a slice of dotted names (the common case:
    /// every name references-only at schema_ver 1). A convenience for the GIT/KN registration.
    pub fn references_only(subsystem: impl Into<String>, names: &[&str]) -> SubsystemTokenList {
        SubsystemTokenList {
            subsystem: subsystem.into(),
            tokens: names.iter().map(|n| RegisteredToken::references_only(*n)).collect(),
        }
    }
}

/// Why a registration into the [`TokenListHarness`] was REJECTED (LOUD, never silent — EI-01 §5).
/// Every variant names the offending subsystem + token so a malformed addition is traced to its
/// source, not silently coerced or dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HarnessError {
    /// A registered name is NOT grammar-conformant (the §6.1 grammar) — the wrapped
    /// [`TaxonomyError`] is the specific rule it broke.
    UngrammaticalToken {
        /// The subsystem whose list carried the bad name.
        subsystem: String,
        /// The offending dotted name.
        token: String,
        /// The specific grammar rule it broke.
        cause: TaxonomyError,
    },
    /// A registered name does NOT carry its subsystem's §6.2 prefix — a subsystem may ONLY register
    /// names under its own prefix (the acyclic-producer invariant; `git` cannot register `ci.*`).
    ForeignPrefix {
        /// The subsystem the list belongs to.
        subsystem: String,
        /// The offending name (whose leading token is not `subsystem`).
        token: String,
    },
    /// The leading `subsystem` token is itself not a canonical §6.2 subsystem token.
    UnknownSubsystem {
        /// The non-canonical subsystem token.
        subsystem: String,
    },
    /// A name appears twice (within the list, or already registered by ANOTHER subsystem) — each name
    /// is minted ONCE; a collision is a contract smell, never silently merged.
    DuplicateToken {
        /// The subsystem registering the duplicate.
        subsystem: String,
        /// The duplicated name.
        token: String,
    },
    /// A `schema_ver` of `0` — a live payload version is `≥ 1` (the envelope's `schema_ver` is `1`-
    /// based, contract 2.1); a `0` is a registration bug.
    ZeroSchemaVer {
        /// The subsystem.
        subsystem: String,
        /// The name with the bad version.
        token: String,
    },
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarnessError::UngrammaticalToken { subsystem, token, cause } => write!(
                f,
                "subsystem `{subsystem}`: registered token `{token}` is ungrammatical: {cause}"
            ),
            HarnessError::ForeignPrefix { subsystem, token } => write!(
                f,
                "subsystem `{subsystem}`: token `{token}` does not carry the `{subsystem}.` prefix \
                 (a subsystem may only register names under its own §6.2 prefix)"
            ),
            HarnessError::UnknownSubsystem { subsystem } => write!(
                f,
                "`{subsystem}` is not a canonical §6.2 subsystem token \
                 (git/ci/issue/knowledge/chat/identity/refs)"
            ),
            HarnessError::DuplicateToken { subsystem, token } => write!(
                f,
                "subsystem `{subsystem}`: token `{token}` is registered more than once \
                 (each name is minted exactly once)"
            ),
            HarnessError::ZeroSchemaVer { subsystem, token } => write!(
                f,
                "subsystem `{subsystem}`: token `{token}` has schema_ver 0 (a live version is ≥ 1)"
            ),
        }
    }
}

impl std::error::Error for HarnessError {}

/// **THE per-subsystem token-list validation HARNESS (contract 2.9).** The Bus-owned registry every
/// subsystem registers its completed dotted-name list into. It holds the registered tokens keyed by
/// `(subsystem, name)`, enforcing — at registration time — the §6.1 grammar, the own-prefix
/// invariant, no cross-subsystem name collision, and a `≥ 1` schema_ver. A malformed list (or a
/// malformed ADDITION to a list) is rejected LOUDLY with the offending token named; a well-formed
/// list is ADMITTED in full.
#[derive(Debug, Default)]
pub struct TokenListHarness {
    /// `name → (subsystem, RegisteredToken)` — the flat registry across all subsystems. A `BTreeMap`
    /// so iteration is deterministic (for the conformance assertions + telemetry). Keyed on the
    /// globally-unique dotted name (no two subsystems may register the same name).
    by_name: BTreeMap<String, (String, RegisteredToken)>,
}

impl TokenListHarness {
    /// A fresh, empty harness (no subsystem registered yet).
    pub fn new() -> TokenListHarness {
        TokenListHarness::default()
    }

    /// **Register a subsystem's COMPLETE list.** Validates EVERY token against the §6.1 grammar + the
    /// own-prefix invariant + no-collision + `≥ 1` schema_ver, then ADMITS the whole list. Returns
    /// the count of tokens admitted on success, or the FIRST [`HarnessError`] (the offending token
    /// named) on rejection. **All-or-nothing**: a list with a malformed token registers NOTHING (the
    /// harness is unchanged), so a half-registered subsystem can never be observed.
    pub fn register(&mut self, list: &SubsystemTokenList) -> Result<usize, HarnessError> {
        // The leading token must itself be a canonical §6.2 subsystem token.
        if !taxonomy::SUBSYSTEM_TOKENS.contains(&list.subsystem.as_str()) {
            return Err(HarnessError::UnknownSubsystem { subsystem: list.subsystem.clone() });
        }
        // Validate the WHOLE list FIRST (all-or-nothing — never a partial registration), tracking
        // intra-list duplicates as we go.
        let prefix = format!("{}.", list.subsystem);
        let mut seen_in_list: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for tok in &list.tokens {
            // Grammar (the one Bus gate).
            taxonomy::validate(&tok.name).map_err(|cause| HarnessError::UngrammaticalToken {
                subsystem: list.subsystem.clone(),
                token: tok.name.clone(),
                cause,
            })?;
            // Own-prefix invariant (the acyclic-producer rule — only your own §6.2 prefix).
            if !tok.name.starts_with(&prefix) {
                return Err(HarnessError::ForeignPrefix {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
            // schema_ver lineage ≥ 1.
            if tok.current_schema_ver == 0 {
                return Err(HarnessError::ZeroSchemaVer {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
            // No intra-list duplicate.
            if !seen_in_list.insert(tok.name.as_str()) {
                return Err(HarnessError::DuplicateToken {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
            // No cross-subsystem collision with an already-registered name.
            if self.by_name.contains_key(&tok.name) {
                return Err(HarnessError::DuplicateToken {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
        }
        // Every token passed — ADMIT the whole list (now infallible; the validation above is total).
        for tok in &list.tokens {
            self.by_name.insert(tok.name.clone(), (list.subsystem.clone(), tok.clone()));
        }
        Ok(list.tokens.len())
    }

    /// Try to add a SINGLE token to an already-registered subsystem (the "reject a malformed
    /// addition" half of the 2.9 gate). Same validation as [`register`](Self::register); a malformed
    /// addition is rejected LOUDLY and the harness is unchanged.
    pub fn add(&mut self, subsystem: &str, tok: RegisteredToken) -> Result<(), HarnessError> {
        let list = SubsystemTokenList { subsystem: subsystem.to_string(), tokens: vec![tok] };
        self.register(&list).map(|_| ())
    }

    /// Is `name` a registered token? (The consumer/producer carriage asserts an emitted name is in
    /// the registry — an unregistered name on the wire is a producer bug.)
    pub fn is_registered(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// The registered token for `name` (its subsystem + schema_ver + payload shape), if any.
    pub fn lookup(&self, name: &str) -> Option<(&str, &RegisteredToken)> {
        self.by_name.get(name).map(|(sub, tok)| (sub.as_str(), tok))
    }

    /// The total number of registered tokens across all subsystems.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// `true` iff no token is registered.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The names registered for `subsystem`, in deterministic (sorted) order.
    pub fn names_for(&self, subsystem: &str) -> Vec<&str> {
        self.by_name
            .iter()
            .filter(|(_, (sub, _))| sub == subsystem)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// The set of subsystems that have registered at least one token (deterministic order).
    pub fn registered_subsystems(&self) -> Vec<&str> {
        let mut subs: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for (sub, _) in self.by_name.values() {
            subs.insert(sub.as_str());
        }
        subs.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small well-formed git list (a representative slice — the real list is
    /// `myelin_git::events::GIT_EVENT_TOKENS`, registered through the CDC against THIS harness).
    fn git_list() -> SubsystemTokenList {
        SubsystemTokenList::references_only(
            "git",
            &["git.ref.updated", "git.pr.opened", "git.pr.merged", "git.repo.snapshot"],
        )
    }

    /// A small well-formed knowledge list (KN's M3 list — page/doc/row + the snapshot reindex name).
    fn kn_list() -> SubsystemTokenList {
        SubsystemTokenList::references_only(
            "knowledge",
            &["knowledge.page.created", "knowledge.page.updated", "knowledge.page.snapshot"],
        )
    }

    /// **The HARNESS ADMITS a subsystem's full well-formed list (the GREEN half of 2.9).**
    #[test]
    fn harness_admits_a_full_well_formed_list() {
        let mut h = TokenListHarness::new();
        assert_eq!(h.register(&git_list()).unwrap(), 4, "the whole git list is admitted");
        assert_eq!(h.register(&kn_list()).unwrap(), 3, "the whole KN list is admitted");
        assert!(h.is_registered("git.ref.updated"));
        assert!(h.is_registered("knowledge.page.snapshot"));
        assert_eq!(h.len(), 7);
        assert_eq!(h.registered_subsystems(), vec!["git", "knowledge"]);
    }

    /// **The HARNESS REJECTS a malformed ADDITION (the RED half of 2.9) — LOUDLY, by the rule.**
    #[test]
    fn harness_rejects_a_malformed_addition() {
        let mut h = TokenListHarness::new();
        h.register(&git_list()).unwrap();

        // present-tense verb
        assert!(matches!(
            h.add("git", RegisteredToken::references_only("git.pr.open")),
            Err(HarnessError::UngrammaticalToken { cause: TaxonomyError::PresentTenseVerb { .. }, .. })
        ));
        // uppercase token
        assert!(matches!(
            h.add("git", RegisteredToken::references_only("git.PR.opened")),
            Err(HarnessError::UngrammaticalToken { cause: TaxonomyError::BadToken { .. }, .. })
        ));
        // The harness is UNCHANGED after the rejected additions (all-or-nothing).
        assert_eq!(h.len(), 4, "a rejected addition leaves the harness unchanged");
        assert!(!h.is_registered("git.pr.open"));
    }

    /// A subsystem may ONLY register names under its OWN §6.2 prefix (the acyclic-producer invariant)
    /// — `git` cannot register a `ci.*` name.
    #[test]
    fn a_subsystem_cannot_register_a_foreign_prefix_name() {
        let mut h = TokenListHarness::new();
        let bad = SubsystemTokenList::references_only("git", &["ci.check.updated"]);
        assert!(matches!(
            h.register(&bad),
            Err(HarnessError::ForeignPrefix { subsystem, token })
                if subsystem == "git" && token == "ci.check.updated"
        ));
        assert!(h.is_empty(), "a list with a foreign-prefix name registers NOTHING (all-or-nothing)");
    }

    /// A non-canonical leading subsystem token is rejected (the leading token must be a §6.2 token).
    #[test]
    fn an_unknown_subsystem_is_rejected() {
        let mut h = TokenListHarness::new();
        let bad = SubsystemTokenList::references_only("billing", &["billing.invoice.created"]);
        assert!(matches!(h.register(&bad), Err(HarnessError::UnknownSubsystem { .. })));
    }

    /// No two subsystems (nor one list twice) may register the same name — each name is minted once.
    #[test]
    fn a_cross_subsystem_name_collision_is_rejected() {
        let mut h = TokenListHarness::new();
        h.register(&SubsystemTokenList::references_only("git", &["git.ref.updated"])).unwrap();
        // The same name registered again (even by the same subsystem) is a duplicate.
        assert!(matches!(
            h.register(&SubsystemTokenList::references_only("git", &["git.ref.updated"])),
            Err(HarnessError::DuplicateToken { .. })
        ));
        // An intra-list duplicate is caught too.
        let dup = SubsystemTokenList::references_only("knowledge", &["knowledge.page.created", "knowledge.page.created"]);
        assert!(matches!(h.register(&dup), Err(HarnessError::DuplicateToken { .. })));
        assert!(h.names_for("knowledge").is_empty(), "the duplicate list registered nothing");
    }

    /// The schema_ver lineage is held + must be ≥ 1; a `0` is a registration bug.
    #[test]
    fn schema_ver_lineage_is_held_and_must_be_at_least_one() {
        let mut h = TokenListHarness::new();
        let list = SubsystemTokenList::new(
            "knowledge",
            vec![RegisteredToken::references_only("knowledge.page.updated").at_schema_ver(3)],
        );
        h.register(&list).unwrap();
        let (sub, tok) = h.lookup("knowledge.page.updated").unwrap();
        assert_eq!(sub, "knowledge");
        assert_eq!(tok.current_schema_ver, 3, "the schema_ver lineage tip is held");

        // schema_ver 0 is rejected.
        let bad = SubsystemTokenList::new(
            "knowledge",
            vec![RegisteredToken::references_only("knowledge.row.updated").at_schema_ver(0)],
        );
        assert!(matches!(h.register(&bad), Err(HarnessError::ZeroSchemaVer { .. })));
    }

    /// The payload-shape descriptor is held per name (references-only / inline-PII / firehose).
    #[test]
    fn payload_shape_descriptor_is_held_per_name() {
        let mut h = TokenListHarness::new();
        let list = SubsystemTokenList::new(
            "chat",
            vec![
                RegisteredToken::references_only("chat.message.created"),
                RegisteredToken::firehose("chat.message.typing"),
            ],
        );
        h.register(&list).unwrap();
        assert_eq!(h.lookup("chat.message.created").unwrap().1.shape, PayloadShape::ReferencesOnly);
        assert_eq!(h.lookup("chat.message.typing").unwrap().1.shape, PayloadShape::EphemeralFirehose);
    }
}
