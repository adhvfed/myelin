pub mod agent_registry;
pub mod agent_session;
pub mod authenticate;
pub mod capability_crypto;
pub mod chat_fragment;
pub mod check_engine;
pub mod ci_fragment;
pub mod delegation;
pub mod delegation_policy;
pub mod expand;
pub mod failstatic_cache;
pub mod git_fragment;
pub mod issue_fragment;
pub mod knowledge_fragment;
pub mod knowledge_rules;
pub mod list_objects;
pub mod lowering;
pub mod machine_auth;
pub mod mint;
pub mod multi_cell;
pub mod namespace;
pub mod oidc;
pub mod principal_store;
pub mod project_store;
pub mod pseudonym_erase;
pub mod pseudonym_store;
pub mod read_replica;
pub mod reverse_index;
pub mod revocation;
pub mod saml;
pub mod ssh_auth;
pub mod tuple_store;
pub mod webauthn;

pub use agent_registry::{
    agent_ref, validate_new_agent, AgentActivation, AgentLifecycleAction, AgentLifecycleOutcome,
    AgentLifecycleRequest, AgentRegistration, AgentRegistryError, NewAgent, PgAgentRegistry,
    EXTERNAL_MCP_RUNTIME, HOSTED_LUNA_RUNTIME, MAX_AGENT_NAME_BYTES, MAX_AGENT_TOOLS,
};
pub use agent_session::{
    agent_run_ref, AgentSession, AgentSessionError, AgentSessionIssuer, AgentSessionRequest,
    AuthorizedAgentSession, ClosedAgentSession, IssuedAgentSession,
    MAX_EXTERNAL_AGENT_RUN_TTL_SECS,
};
pub use authenticate::{
    scheme, AuthTelemetry, CredentialVerifier, HumanSsoAuthenticator, IdorCounters,
    StructuralVerifier, VerifiedAssertion,
};
pub use capability_crypto::{
    attenuate, CapabilityMintSpec, CellAnchorSet, CellTokenAuthority, CellTrustAnchor, DpopBinding,
    DpopClientKey, DpopReplayGuard, PasetoCapabilitySigner, PasetoCapabilityVerifier,
};
pub use chat_fragment::{
    channel_fragment, chat_fragment_defs, message_fragment, unfurl_fragment,
    MEMBER as CHANNEL_MEMBER, READ as CHANNEL_READ, TARGET as UNFURL_TARGET, VIEW as MESSAGE_VIEW,
};
pub use check_engine::{eval_caveat, eval_caveat_predicate, CheckEngine, MAX_REWRITE_DEPTH};
pub use ci_fragment::{
    ci_fragment, IS_UNTRUSTED_FORK, READ as CI_READ, SECRET_DIRECT_READER, TRIGGER as CI_TRIGGER,
    VIEW as CI_VIEW,
};
pub use delegation::{
    authority_of, effective_policy_of, DelegationAlgebra, DelegationInput, IntersectionProof,
    EFFECTIVE_GRANT_CARRIER,
};
pub use delegation_policy::{
    DelegationPolicyError, DelegationPolicySource, DelegationPolicyVersionCursor,
    DelegationRunPolicyCursor, ResolvedDelegationPolicy,
};
pub use expand::Expand;
pub use failstatic_cache::{
    CacheTelemetry, CachedDecision, CoarseGrant, FailStaticCache, Served, FRESH_TTL_SECS, S6_STORE,
};
pub use git_fragment::{
    compile_codeowners, git_fragment, CodeownersRule, APPROVE_UNTRUSTED_CI, CODE_OWNER,
    PROTECTED_PUSH,
};
pub use issue_fragment::{
    field_view_caveat as issue_field_view_caveat, issue_fragment_defs, transition_caveat, APPROVER,
    ASSIGNEE, CONFIDENTIAL, CONFIDENTIAL_GRANT, MANAGE as ISSUE_MANAGE,
    PERFORM_TRANSITION as ISSUE_PERFORM_TRANSITION, TRANSITION_PERM as ISSUE_TRANSITION_PERM,
    VIEW as ISSUE_VIEW, VIEW_FIELD as ISSUE_VIEW_FIELD,
};
pub use knowledge_fragment::{
    field_view_caveat, knowledge_fragment, DIRECT_BLOCK, DIRECT_EDITOR, DIRECT_READER,
    EDIT as KN_EDIT, READ as KN_READ, VIEW_FIELD,
};
pub use list_objects::{ListObjects, DEFAULT_IDS_CARDINALITY_CAP};
pub use lowering::{
    fall_back_to_check, is_fall_back, lower, watermark_verdict, AuthzJoin, BoundParam, Lowered,
    WatermarkVerdict,
};
pub use machine_auth::{
    scheme as machine_scheme, Authority, CapabilityAuthenticator, CapabilityToken,
    CredentialAudience, CredentialContext, CredentialPurpose, DpopState, MachineKind,
    RequestIdentity, StructuralTokenVerifier, TokenVerifier, VerifiedCapabilityContext,
};
pub use mint::{
    expires_at_of, run_token_jti, CiJobAuthorizationError, MintError, RevocationProof,
    RunTokenMinter, StructuralTokenSigner, TokenSignRequest, TokenSigner, RUN_GRANT_RELATION,
    SELFHOSTED_GRANT_PREFIX,
};
pub use multi_cell::{
    CellPartition, CrossCellAudit, CrossCellGrant, CrossCellResolution, MigrationReceipt,
    MultiCellAuthority, MultiCellDsrReceiptSet,
};
pub use namespace::{
    core_hierarchy, AdmitReject, FragmentDef, NamespaceEngine, PermissionRule, Userset,
    MAX_RULE_DEPTH, WATCHER_RELATION,
};
pub use oidc::{
    oidc_login_material, JwkKey, JwkSet, OidcConfig, OidcVerifier, ReplayGuard,
    SchemeDispatchVerifier,
};
pub use principal_store::{
    PrincipalCredentialProvision, PrincipalError, PrincipalProfile, PrincipalRow, PrincipalStore,
    ProfileRef, S1_HOLDER, S1_TABLE,
};
pub use project_store::{
    project_ref, validate_new_project, NewProject, PgProjectStore, Project, ProjectCreation,
    ProjectError, MAX_PROJECT_NAME_BYTES, MAX_PROJECT_PREFIX_BYTES, PROJECT_WRITER_RELATION,
    VISIBLE_PROJECTS_CTE,
};
pub use pseudonym_erase::{
    ErasureLedgerEntry, ErasureReceipt, PseudonymEraseError, PseudonymErasureLedger,
    ReErasureReceipt, ERASURE_LEDGER,
};
pub use pseudonym_store::{PseudonymError, PseudonymRow, PseudonymStore, S2_HOLDER, S2_TABLE};
pub use read_replica::{
    AuthzReadReplica, ReadRoute, ReplicaRow, ReplicaTelemetry, ReplicaWriteRejected, S5_HOLDER,
    S5_TABLE,
};
pub use reverse_index::{
    ReverseIndex, ReverseIndexConsumer, ReverseRow, S8_CONSUMER, S8_HOLDER, S8_TABLE,
};
pub use revocation::{
    RevocationEntry, RevocationStore, RevocationTelemetry, RevokedKind, RunTokenState,
    REVOCATION_SLA_SECS, S7_TABLE,
};
pub use ssh_auth::{
    encode_ssh_credential_material, signed_payload, ssh_fingerprint, Challenge, ChallengeGuard,
    KeyBindingIndex, KeyBindingResolver, PrincipalStoreKeyBindings, RegisteredKey, SshVerifier,
};
pub use tuple_store::{run_grant_expiry, StoredTuple, TupleStore, WriteError, S3_HOLDER, S3_TABLE};
pub use webauthn::{
    encode_assertion_material, encode_registration_material,
    ChallengeGuard as WebauthnChallengeGuard, CoseKey, CredentialBindingIndex, WebauthnConfig,
    WebauthnVerifier,
};

use myelin_events::OutboxStore;
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Decision, IdentityService, ListObjectsResult,
    ObjectType, Permission, Principal, Zookie,
};
use myelin_substrate::{
    boot, AppSpec, Authorizer, Config, CriticalDependencies, InternalRpc, Migration, Migrations,
    PublicRoutes, ServeError, ServeHandle, StoreManifest,
};
use myelin_tenancy::ArtifactRef;

pub const SERVICE_NAME: &str = "identity";

pub fn identity_service_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0100_identity_schema_marker",
            "CREATE TABLE IF NOT EXISTS identity_schema_marker (applied_at TEXT)",
        ),
        Migration::plain(
            "0101_s1_principal",
            "CREATE TABLE IF NOT EXISTS principal (\
                 tenant TEXT NOT NULL, \
                 region TEXT NOT NULL, \
                 principal_id TEXT NOT NULL, \
                 kind TEXT NOT NULL, \
                 profile_ref TEXT, \
                 data_role TEXT NOT NULL, \
                 status TEXT NOT NULL, \
                 PRIMARY KEY (tenant, region, principal_id))",
        ),
        Migration::plain(
            "0102_s7_revocation",
            "CREATE TABLE IF NOT EXISTS revocation (\
                 tenant TEXT NOT NULL, \
                 region TEXT NOT NULL, \
                 kind TEXT NOT NULL, \
                 handle TEXT NOT NULL, \
                 revoked_at TEXT NOT NULL, \
                 expires_at TEXT, \
                 PRIMARY KEY (tenant, region, kind, handle))",
        ),
        Migration::plain(
            "0103_s2_pseudonym_map",
            "CREATE TABLE IF NOT EXISTS pseudonym_map (\
                 tenant TEXT NOT NULL, \
                 region TEXT NOT NULL, \
                 principal_id TEXT NOT NULL, \
                 pseudonym TEXT NOT NULL, \
                 real_id_key_ref TEXT NOT NULL, \
                 PRIMARY KEY (tenant, region, principal_id), \
                 UNIQUE (tenant, region, pseudonym))",
        ),
    ])
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FailClosedCheck;

impl FailClosedCheck {
    pub fn new() -> FailClosedCheck {
        FailClosedCheck
    }
}

impl IdentityService for FailClosedCheck {
    fn authenticate(
        &self,
        _credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented(
            "authenticate → P-ID-06/07 (M1); the shell wires the slot, not the body",
        ))
    }

    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        Ok(Decision::Deny)
    }

    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented(
            "list_objects → P-ID-11/12 (M1); the shell wires the slot, not the body",
        ))
    }

    fn list_subjects(
        &self,
        _object: &myelin_identity::ObjectId,
        _permission: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        Err(AuthzError::NotYetImplemented(
            "list_subjects → P-ID-13 (M1)",
        ))
    }

    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &myelin_identity::ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(AuthzError::NotYetImplemented("explain → P-ID-13 (M1)"))
    }

    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("delegation → P-ID-17 (M1)"))
    }

    fn write_tuples(
        &self,
        _deltas: &[myelin_identity::TupleDelta],
        _precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        Err(AuthzError::NotYetImplemented("write_tuples → P-ID-08 (M1)"))
    }

    fn mint_run_token(
        &self,
        _agent_id: &myelin_identity::PrincipalId,
        _run_id: &myelin_identity::RunId,
        _delegation_caveats: &myelin_identity::DelegationCaveats,
        _ttl: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented(
            "mint_run_token → P-ID-18 (M1)",
        ))
    }

    fn revoke(&self, _target: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("revoke → P-ID-14 (M1)"))
    }

    fn resolve_pseudonym(
        &self,
        _subject: &myelin_identity::PrincipalId,
        _tenant: &myelin_tenancy::TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented(
            "resolve_pseudonym → P-ID-19 (M1)",
        ))
    }

    fn erase(&self, _subject: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("erase → P-ID-20 (M1)"))
    }

    fn admit_fragment(
        &self,
        _fragment: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented(
            "admit_fragment → P-ID-10 (M1)",
        ))
    }
}

pub struct CheckAuthorizer<S: IdentityService + Send + Sync> {
    inner: S,
}

impl<S: IdentityService + Send + Sync> CheckAuthorizer<S> {
    pub fn new(inner: S) -> CheckAuthorizer<S> {
        CheckAuthorizer { inner }
    }
}

impl<S: IdentityService + Send + Sync> Authorizer for CheckAuthorizer<S> {
    fn authorize(&self, subject: &Principal, action: &str) -> bool {
        let permission = Permission(action.to_string());
        let object = ArtifactRef(format!(
            "myelin://{}/identity/action/{}",
            subject.tenant.0, action
        ));
        let at = Consistency {
            at_least: myelin_identity::Zookie(String::new()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        matches!(
            self.inner.check(subject, &permission, &object, &at, None),
            Ok(Decision::Allow)
        )
    }
}

#[derive(Clone)]
pub struct StoreBackedCheck {
    engine: CheckEngine,
    tuples: TupleStore,
    index: ReverseIndex,
    namespace: std::sync::Arc<std::sync::Mutex<NamespaceEngine>>,
    revocations: revocation::RevocationStore,
    read_replica: read_replica::AuthzReadReplica,
    minter: mint::RunTokenMinter,
    kms: std::sync::Arc<myelin_storage::KmsEngine>,
    pseudonyms: PseudonymStore,
    erasure_ledger: pseudonym_erase::PseudonymErasureLedger,
    cell_authority: std::sync::Arc<CellTokenAuthority>,
}

impl StoreBackedCheck {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(tuples: TupleStore) -> StoreBackedCheck {
        StoreBackedCheck::with_index(tuples, ReverseIndex::new())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_index(tuples: TupleStore, index: ReverseIndex) -> StoreBackedCheck {
        let revocations = revocation::RevocationStore::new();
        let cell_authority = std::sync::Arc::new(CellTokenAuthority::generate());
        let signer = std::sync::Arc::new(PasetoCapabilitySigner::new(cell_authority.clone()));
        let minter = mint::RunTokenMinter::with_signer_and_tuples(
            revocations.clone(),
            Some(tuples.clone()),
            signer,
        );
        let kms = std::sync::Arc::new(myelin_storage::KmsEngine::new());
        let pseudonyms = PseudonymStore::new(kms.clone());
        let erasure_ledger = pseudonym_erase::PseudonymErasureLedger::new();
        StoreBackedCheck::with_kms(
            tuples,
            index,
            revocations,
            minter,
            kms,
            cell_authority,
            pseudonyms,
            erasure_ledger,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_kms(
        tuples: TupleStore,
        index: ReverseIndex,
        revocations: revocation::RevocationStore,
        minter: mint::RunTokenMinter,
        kms: std::sync::Arc<myelin_storage::KmsEngine>,
        cell_authority: std::sync::Arc<CellTokenAuthority>,
        pseudonyms: PseudonymStore,
        erasure_ledger: pseudonym_erase::PseudonymErasureLedger,
    ) -> StoreBackedCheck {
        StoreBackedCheck {
            engine: CheckEngine::new(tuples.clone()),
            tuples,
            index,
            namespace: std::sync::Arc::new(std::sync::Mutex::new(
                NamespaceEngine::with_core_hierarchy(),
            )),
            revocations,
            read_replica: read_replica::AuthzReadReplica::new(),
            minter,
            pseudonyms,
            erasure_ledger,
            kms,
            cell_authority,
        }
    }

    pub fn with_pg(
        provider: myelin_storage::SubstrateProvider,
        kms: std::sync::Arc<myelin_storage::KmsEngine>,
        cell_authority: std::sync::Arc<CellTokenAuthority>,
        handle: tokio::runtime::Handle,
    ) -> StoreBackedCheck {
        let tuples = TupleStore::with_pg(
            myelin_storage::DurableTupleBacking::new(provider.clone()),
            handle.clone(),
        );
        let revocations = RevocationStore::with_pg(
            myelin_storage::DurableRevocationBacking::new(provider.clone()),
            handle.clone(),
        );
        let pseudonyms = PseudonymStore::with_pg(
            kms.clone(),
            myelin_storage::DurablePseudonymBacking::new(provider.clone()),
            handle.clone(),
        );
        let erasure_ledger = pseudonym_erase::PseudonymErasureLedger::with_pg(
            myelin_storage::DurableErasureLedgerBacking::new(provider),
            handle,
        );
        let signer = std::sync::Arc::new(PasetoCapabilitySigner::new(cell_authority.clone()));
        let minter = mint::RunTokenMinter::with_signer_and_tuples(
            revocations.clone(),
            Some(tuples.clone()),
            signer,
        );
        StoreBackedCheck::with_kms(
            tuples,
            ReverseIndex::new(),
            revocations,
            minter,
            kms,
            cell_authority,
            pseudonyms,
            erasure_ledger,
        )
    }

    pub fn token_trust_anchor(&self) -> CellTrustAnchor {
        self.cell_authority.trust_anchor()
    }

    pub fn introspect_run_token(
        &self,
        scheme: &str,
        token: &myelin_identity::RunToken,
    ) -> myelin_identity::Result<CapabilityToken> {
        use machine_auth::TokenVerifier;
        PasetoCapabilityVerifier::new(self.cell_authority.trust_anchor()).verify(
            &myelin_identity::Credential {
                scheme: scheme.to_string(),
                material: token.token.clone(),
            },
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn introspect_run_token_at(
        &self,
        scheme: &str,
        token: &myelin_identity::RunToken,
        now: &myelin_events::Timestamp,
    ) -> myelin_identity::Result<CapabilityToken> {
        use machine_auth::TokenVerifier;
        let now = chrono::DateTime::parse_from_rfc3339(&now.0)
            .map_err(|error| {
                myelin_identity::AuthzError::BadRequest(format!(
                    "malformed deterministic introspection instant: {error}"
                ))
            })?
            .timestamp();
        PasetoCapabilityVerifier::new(self.cell_authority.trust_anchor())
            .with_clock(move || now)
            .verify(&myelin_identity::Credential {
                scheme: scheme.to_string(),
                material: token.token.clone(),
            })
    }

    pub fn revocations(&self) -> &revocation::RevocationStore {
        &self.revocations
    }

    pub fn tuples(&self) -> &TupleStore {
        &self.tuples
    }

    pub fn revoke_in(
        &self,
        scope: &myelin_storage::TenantScope,
        target: &myelin_identity::RevokeTarget,
        now: myelin_events::Timestamp,
    ) {
        self.revocations.revoke(scope, target, now);
    }

    pub fn disable_principal_in(
        &self,
        scope: &myelin_storage::TenantScope,
        principal: &myelin_identity::PrincipalId,
        now: myelin_events::Timestamp,
    ) {
        self.revocations.disable_principal(scope, principal, now);
    }

    pub fn pseudonyms(&self) -> &PseudonymStore {
        &self.pseudonyms
    }

    pub fn erasure_ledger(&self) -> &pseudonym_erase::PseudonymErasureLedger {
        &self.erasure_ledger
    }

    pub fn kms(&self) -> &std::sync::Arc<myelin_storage::KmsEngine> {
        &self.kms
    }

    pub fn resolve_pseudonym_in(
        &self,
        scope: &myelin_storage::TenantScope,
        subject: &myelin_identity::PrincipalId,
    ) -> Result<myelin_identity::PseudonymHandle, pseudonym_erase::PseudonymEraseError> {
        match self
            .pseudonyms
            .try_mapping_of(scope, subject)
            .map_err(pseudonym_erase::PseudonymEraseError::from)?
        {
            Some(row) => Ok(row.pseudonym),
            None => {
                if self
                    .erasure_ledger
                    .try_is_erased(scope, subject)
                    .map_err(pseudonym_erase::PseudonymEraseError::from)?
                {
                    Err(pseudonym_erase::PseudonymEraseError::Erased {
                        subject: subject.0.clone(),
                    })
                } else {
                    Err(pseudonym_erase::PseudonymEraseError::NoMapping {
                        subject: subject.0.clone(),
                    })
                }
            }
        }
    }

    pub fn erase_in(
        &self,
        scope: &myelin_storage::TenantScope,
        subject: &myelin_identity::PrincipalId,
        now: myelin_events::Timestamp,
    ) -> pseudonym_erase::ErasureReceipt {
        let dek_class = PseudonymStore::subject_dek_class(subject);
        let (dek_destroyed, row_shredded, shredded_class) = pseudonym_erase::EraseEngine::shred(
            &self.pseudonyms,
            &self.kms,
            scope,
            subject,
            &dek_class,
        );
        self.revocations
            .disable_principal(scope, subject, now.clone());
        self.erasure_ledger
            .record(scope, subject, dek_class.clone(), now.clone());
        pseudonym_erase::ErasureReceipt::for_erase(
            subject.clone(),
            scope.tenant().clone(),
            scope.region().clone(),
            shredded_class,
            dek_destroyed,
            row_shredded,
            now,
        )
    }

    pub fn re_erase_after_restore(
        &self,
        scope: &myelin_storage::TenantScope,
        now: myelin_events::Timestamp,
    ) -> Result<pseudonym_erase::ReErasureReceipt, PseudonymError> {
        let entries = self.erasure_ledger.try_entries_in(scope)?;
        let mut per_subject = Vec::with_capacity(entries.len());
        let mut resurrected = 0usize;
        for entry in &entries {
            let live_before = self
                .pseudonyms
                .try_resolve_subject(scope, &entry.subject)?
                .is_some();
            if live_before {
                resurrected += 1;
            }
            let (dek_destroyed, row_shredded, shredded_class) = pseudonym_erase::EraseEngine::shred(
                &self.pseudonyms,
                &self.kms,
                scope,
                &entry.subject,
                &entry.dek_class,
            );
            self.revocations
                .disable_principal(scope, &entry.subject, now.clone());
            per_subject.push(pseudonym_erase::ErasureReceipt::for_erase(
                entry.subject.clone(),
                scope.tenant().clone(),
                scope.region().clone(),
                shredded_class,
                dek_destroyed,
                row_shredded,
                now.clone(),
            ));
        }
        let mut still_resolvable = 0usize;
        for entry in &entries {
            if self
                .pseudonyms
                .try_resolve_subject(scope, &entry.subject)?
                .is_some()
            {
                still_resolvable += 1;
            }
        }
        Ok(pseudonym_erase::ReErasureReceipt {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            re_erased: entries.len(),
            resurrected: still_resolvable,
            pre_pass_resurrected: 0,
            per_subject,
            ran_at: now,
        }
        .with_pre_pass_resurrected(resurrected))
    }

    pub fn index(&self) -> &ReverseIndex {
        &self.index
    }

    pub fn current_zookie(&self) -> Zookie {
        self.tuples.current_zookie()
    }

    pub fn read_replica(&self) -> &read_replica::AuthzReadReplica {
        &self.read_replica
    }

    pub fn route_read(&self, at: &Consistency) -> read_replica::ReadRoute {
        self.read_replica.route(at)
    }

    pub fn failstatic_cache(
        &self,
        revocation_sla_secs: myelin_substrate::Seconds,
        threshold: &myelin_substrate::thresholds::FailStaticThreshold,
    ) -> Result<failstatic_cache::FailStaticCache, myelin_substrate::FailStaticError> {
        failstatic_cache::FailStaticCache::try_new(
            revocation_sla_secs,
            threshold,
            self.revocations.clone(),
        )
    }

    pub fn failstatic_cache_with_clock<C: myelin_substrate::Clock>(
        &self,
        revocation_sla_secs: myelin_substrate::Seconds,
        threshold: &myelin_substrate::thresholds::FailStaticThreshold,
        clock: C,
    ) -> Result<failstatic_cache::FailStaticCache<C>, myelin_substrate::FailStaticError> {
        failstatic_cache::FailStaticCache::try_new_with_clock(
            revocation_sla_secs,
            threshold,
            self.revocations.clone(),
            clock,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn check_failstatic<C: myelin_substrate::Clock>(
        &self,
        s6: &failstatic_cache::FailStaticCache<C>,
        scope: &myelin_storage::TenantScope,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
        now: &myelin_events::Timestamp,
        source_ok: bool,
    ) -> failstatic_cache::CachedDecision {
        let question = format!("{}@{}", permission.0, object.0);
        s6.check_cached(scope, &subject.principal_id, &question, at, now, || {
            if !source_ok {
                return Err(myelin_substrate::ServeError("identity authz hiccup".into()));
            }
            match self.check(subject, permission, object, at, caveat) {
                Ok(decision) => Ok(decision),
                Err(_) => Ok(Decision::Deny),
            }
        })
    }

    pub fn admit_fragment_def(&self, frag: &FragmentDef) -> myelin_identity::FragmentAdmit {
        self.namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(frag)
    }

    pub fn admit_git_fragment(&self) -> Vec<myelin_identity::FragmentAdmit> {
        let mut ns = self.namespace.lock().unwrap_or_else(|e| e.into_inner());
        git_fragment::git_fragment()
            .iter()
            .map(|def| ns.admit(def))
            .collect()
    }

    pub fn admit_knowledge_fragment(&self) -> Vec<myelin_identity::FragmentAdmit> {
        let mut ns = self.namespace.lock().unwrap_or_else(|e| e.into_inner());
        knowledge_fragment::knowledge_fragment()
            .iter()
            .map(|def| ns.admit(def))
            .collect()
    }

    pub fn admit_ci_fragment(&self) -> Vec<myelin_identity::FragmentAdmit> {
        let mut ns = self.namespace.lock().unwrap_or_else(|e| e.into_inner());
        ci_fragment::ci_fragment()
            .iter()
            .map(|def| ns.admit(def))
            .collect()
    }

    pub fn admit_issue_fragment(&self) -> Vec<myelin_identity::FragmentAdmit> {
        let mut ns = self.namespace.lock().unwrap_or_else(|e| e.into_inner());
        issue_fragment::issue_fragment_defs()
            .iter()
            .map(|def| ns.admit(def))
            .collect()
    }

    pub fn admit_chat_fragment(&self) -> Vec<myelin_identity::FragmentAdmit> {
        let mut ns = self.namespace.lock().unwrap_or_else(|e| e.into_inner());
        chat_fragment::chat_fragment_defs()
            .iter()
            .map(|def| ns.admit(def))
            .collect()
    }

    pub fn namespace(&self) -> NamespaceEngine {
        self.namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn delegation_in(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &delegation::DelegationInput,
    ) -> myelin_identity::EffectivePolicy {
        delegation::DelegationAlgebra::new().delegation(agent, trigger_actor, input)
    }

    pub fn delegation_proved_in(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &delegation::DelegationInput,
    ) -> (
        myelin_identity::EffectivePolicy,
        delegation::IntersectionProof,
    ) {
        delegation::DelegationAlgebra::new().delegation_proved(agent, trigger_actor, input)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn delegation_with_check_in(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &delegation::DelegationInput,
        scope: &myelin_storage::TenantScope,
        required_grant: &str,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
    ) -> Decision {
        delegation::DelegationAlgebra::with_check(self.engine.clone()).delegation_with_check(
            agent,
            trigger_actor,
            input,
            scope,
            required_grant,
            permission,
            object,
            at,
        )
    }

    pub fn run_token_minter(&self) -> &mint::RunTokenMinter {
        &self.minter
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mint_run_token_in(
        &self,
        scope: &myelin_storage::TenantScope,
        agent_id: &myelin_identity::PrincipalId,
        run_id: &myelin_identity::RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &delegation::DelegationInput,
        delegation_caveats: &myelin_identity::DelegationCaveats,
        kind: MachineKind,
        ttl: &myelin_identity::FailStaticBound,
        now: &myelin_events::Timestamp,
    ) -> Result<myelin_identity::RunToken, mint::MintError> {
        self.minter.mint_run_token(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input,
            delegation_caveats,
            kind,
            ttl,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mint_run_token_from_resolved_policy_in(
        &self,
        scope: &myelin_storage::TenantScope,
        agent_id: &myelin_identity::PrincipalId,
        run_id: &myelin_identity::RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        resolved: &delegation_policy::ResolvedDelegationPolicy,
        delegation_caveats: &myelin_identity::DelegationCaveats,
        kind: MachineKind,
        ttl: &myelin_identity::FailStaticBound,
        now: &myelin_events::Timestamp,
    ) -> Result<myelin_identity::RunToken, mint::MintError> {
        self.minter.mint_from_resolved_policy(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            resolved,
            delegation_caveats,
            kind,
            ttl,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn re_mint_run_token_in(
        &self,
        scope: &myelin_storage::TenantScope,
        agent_id: &myelin_identity::PrincipalId,
        run_id: &myelin_identity::RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input_as_of_resume: &delegation::DelegationInput,
        delegation_caveats: &myelin_identity::DelegationCaveats,
        kind: MachineKind,
        ttl: &myelin_identity::FailStaticBound,
        now_resume: &myelin_events::Timestamp,
    ) -> Result<myelin_identity::RunToken, mint::MintError> {
        self.minter.re_mint_on_resume(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input_as_of_resume,
            delegation_caveats,
            kind,
            ttl,
            now_resume,
        )
    }

    pub fn tear_down_run_token_in(
        &self,
        scope: &myelin_storage::TenantScope,
        token: &myelin_identity::RunToken,
        now: &myelin_events::Timestamp,
    ) {
        self.minter.teardown(scope, token, now);
    }

    pub fn list_subjects_in(
        &self,
        scope: &myelin_storage::TenantScope,
        object: &myelin_identity::ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> myelin_identity::SubjectTree {
        let namespace = self
            .namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let expand = Expand::new(self.tuples.clone(), namespace, self.index.clone());
        let object_type = ObjectType(infer_object_type(&object.0));
        expand.list_subjects(scope, object, &object_type, permission, at)
    }

    pub fn list_watchers_in(
        &self,
        scope: &myelin_storage::TenantScope,
        object: &myelin_identity::ObjectId,
        at: &Consistency,
    ) -> myelin_identity::SubjectTree {
        self.list_subjects_in(scope, object, &Permission(WATCHER_RELATION.to_string()), at)
    }

    pub fn explain_in(
        &self,
        scope: &myelin_storage::TenantScope,
        subject: &myelin_identity::PrincipalId,
        permission: &Permission,
        object: &myelin_identity::ObjectId,
        at: &Consistency,
    ) -> myelin_identity::RewriteTrace {
        let namespace = self
            .namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let expand = Expand::new(self.tuples.clone(), namespace, self.index.clone());
        let object_type = ObjectType(infer_object_type(&object.0));
        expand.explain(scope, subject, object, &object_type, permission, at)
    }
}

fn infer_object_type(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

impl IdentityService for StoreBackedCheck {
    fn authenticate(
        &self,
        _credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented(
            "authenticate → P-ID-06/07 (M1)",
        ))
    }

    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        let scope =
            myelin_storage::TenantScope::from_verified_token(subject, subject.region.clone());

        let revoke_target = myelin_identity::RevokeTarget::Principal(subject.principal_id.clone());
        if self.revocations.is_revoked(
            &scope,
            &revoke_target,
            &myelin_events::Timestamp(String::new()),
        ) {
            return Ok(Decision::Deny);
        }

        let object_type = namespace::type_of_object_ref(object);
        let granted = self
            .namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .permits(
                &self.engine,
                &scope,
                subject,
                &object_type,
                &permission.0,
                object,
                at,
            );
        if !granted {
            return Ok(Decision::Deny);
        }
        match caveat {
            None => Ok(Decision::Allow),
            Some(cav) => Ok(check_engine::eval_caveat(cav)),
        }
    }

    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        let scope =
            myelin_storage::TenantScope::from_verified_token(subject, subject.region.clone());
        let namespace = self
            .namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let lo = ListObjects::new(self.tuples.clone(), namespace, self.index.clone());
        Ok(lo.list_objects(&scope, subject, permission, ty, at))
    }

    fn list_subjects(
        &self,
        _object: &myelin_identity::ObjectId,
        _permission: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        Err(AuthzError::NotYetImplemented(
            "list_subjects (ABI, scope-less) → use StoreBackedCheck::list_subjects_in (P-ID-13); a \
             tenant-less expand is never served (the tenant-predicate floor)",
        ))
    }

    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &myelin_identity::ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(AuthzError::NotYetImplemented(
            "explain (ABI, scope-less) → use StoreBackedCheck::explain_in (P-ID-13)",
        ))
    }

    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented(
            "delegation (ABI, policy-less) → use StoreBackedCheck::delegation_in (P-ID-17); the \
             monotone intersection needs the conjunct policy sets the credentials carry, which the \
             scope-less ABI method cannot supply (never a fabricated EffectivePolicy)",
        ))
    }

    fn write_tuples(
        &self,
        _deltas: &[myelin_identity::TupleDelta],
        _precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        Err(AuthzError::NotYetImplemented(
            "write_tuples → TupleStore::write_tuples (P-ID-08)",
        ))
    }

    fn mint_run_token(
        &self,
        _agent_id: &myelin_identity::PrincipalId,
        _run_id: &myelin_identity::RunId,
        _delegation_caveats: &myelin_identity::DelegationCaveats,
        _ttl: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented(
            "mint_run_token (ABI, scope-/policy-less) → use StoreBackedCheck::mint_run_token_in \
             (P-ID-18); the mint applies the monotone delegation intersection over the conjunct \
             policy sets the credentials carry + registers the expires_at == run-life TTL in the \
             verified (tenant, region) partition, which the scope-less ABI method cannot supply \
             (never a fabricated/over-broad RunToken)",
        ))
    }

    fn revoke(&self, _target: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented(
            "revoke (ABI, scope-less) → use StoreBackedCheck::revoke_in (P-ID-14); a revoke writes a \
             (tenant, region) partition and must carry a verified scope (the tenant-predicate floor)",
        ))
    }

    fn resolve_pseudonym(
        &self,
        _subject: &myelin_identity::PrincipalId,
        _tenant: &myelin_tenancy::TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented(
            "resolve_pseudonym (ABI, region-less) → use StoreBackedCheck::resolve_pseudonym_in \
             (P-ID-20); S2 is (tenant, region)-partitioned and the read must carry a verified \
             (tenant, region) scope (the tenant-predicate floor)",
        ))
    }

    fn erase(&self, _subject: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented(
            "erase (ABI, scope-less) → use StoreBackedCheck::erase_in (P-ID-20); an erase shreds a \
             (tenant, region) partition row + records the PII-free erasure ledger and must carry a \
             verified scope (the tenant-predicate floor)",
        ))
    }

    fn admit_fragment(
        &self,
        fragment: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Ok(self
            .namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit_abi(fragment))
    }
}

pub fn identity_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: identity_service_migrations(),
        hot_tables: myelin_substrate::HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: myelin_substrate::OutboxSpec::external_relay(outbox),
        critical: CriticalDependencies::default(),
    }
}

pub fn internal_authorizer() -> CheckAuthorizer<FailClosedCheck> {
    CheckAuthorizer::new(FailClosedCheck::new())
}

pub fn boot_identity(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(identity_app_spec(config, outbox))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_substrate::{serve, Readiness, Startup, Surface};
    use myelin_tenancy::TenantId;

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )
    }

    #[test]
    fn identity_shell_boots_and_three_ports_bind() {
        let handle =
            boot_identity(Config::default(), OutboxStore::new()).expect("the identity shell boots");
        assert_eq!(handle.name(), "identity");
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        let surface = myelin_substrate::MetricsHealthSurface::new(
            CriticalDependencies::new(["oltp"]),
            myelin_substrate::HealthTable::new(),
        );
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness is FALSE until migrations apply (the migrate-complete gate)"
        );
        assert!(
            r.startup_incomplete,
            "the not-ready reason names the startup (pre-migrate) gate"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            myelin_substrate::Liveness::Up,
            "liveness ≠ readiness: a booting instance is not-killed (liveness stays Up)"
        );

        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "after migrate-complete the readiness gate lifts → ready"
        );
    }

    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle = boot_identity(Config::default(), OutboxStore::new()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted identity instance (migrations applied, deps up) is ready"
        );
    }

    #[test]
    fn stubbed_check_fail_closes_to_deny() {
        let slot = FailClosedCheck::new();
        let at = Consistency {
            at_least: myelin_identity::Zookie("z".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        let d = slot.check(
            &principal(),
            &Permission("read".into()),
            &ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            &at,
            None,
        );
        assert_eq!(
            d,
            Ok(Decision::Deny),
            "the un-wired check slot denies (fail-closed)"
        );
    }

    #[test]
    fn internal_surface_re_authorizes_against_fail_closed_check() {
        let surface = myelin_substrate::InternalSurface::new(internal_authorizer());
        let r = surface.handle(&principal(), "issues.read");
        assert!(
            matches!(
                r,
                Err(myelin_substrate::InternalReject::Unauthorized { .. })
            ),
            "the internal-RPC call is re-authorized against the fail-closed check and denied"
        );
    }

    #[test]
    fn real_check_engine_swaps_in_behind_the_same_authorizer_seam() {
        use myelin_events::{OutboxStore, Timestamp};
        use myelin_identity::{ObjectId, RelName, RelationTuple, TupleDelta};
        use myelin_storage::TenantScope;

        let alice = Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        let scope = TenantScope::from_verified_token(&alice, alice.region.clone());

        let store = TupleStore::new(OutboxStore::new());
        let admin = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        store
            .write_tuples(
                &scope,
                &admin,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("action:issues.read".into()),
                    relation: RelName("issues.read".into()),
                    subject: PrincipalId("p:alice".into()),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-06-19T00:00:00Z".into()),
            )
            .expect("grant");

        let surface = myelin_substrate::InternalSurface::new(CheckAuthorizer::new(
            StoreBackedCheck::new(store.clone()),
        ));
        assert!(
            surface.handle(&alice, "issues.read").is_ok(),
            "the real engine allows the granted relation through the SAME authorizer seam"
        );
        let bob = Principal::stub(
            PrincipalId("p:bob".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        assert!(
            matches!(
                surface.handle(&bob, "issues.read"),
                Err(myelin_substrate::InternalReject::Unauthorized { .. })
            ),
            "an un-granted subject is denied through the same seam (fail-closed)"
        );
    }

    #[test]
    fn s5_routes_default_consistency_reads_and_bypasses_strong_reads() {
        use myelin_events::OutboxStore;
        use myelin_storage::TenantScope;

        let store = TupleStore::new(OutboxStore::new());
        let slot = StoreBackedCheck::new(store);
        let acme = TenantScope::from_verified_token(&principal(), principal().region.clone());

        slot.read_replica().replicate(
            &acme,
            "add",
            read_replica::ReplicaRow {
                key: "p:alice".into(),
                value: "active".into(),
            },
            5,
        );

        let stale = Consistency {
            at_least: myelin_identity::Zookie(String::new()),
            mode: myelin_identity::ConsistencyMode::BoundedStale,
        };
        assert!(
            slot.route_read(&stale).is_replica(),
            "a default-consistency read is served from S5"
        );
        assert!(
            slot.read_replica().read(&acme, "p:alice").is_some(),
            "the replicated row is served off the stale-tolerant replica"
        );

        let strong = Consistency {
            at_least: myelin_identity::Zookie("zk-00000000000000000005".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        assert!(
            slot.route_read(&strong).is_primary(),
            "a zookie-stamped read bypasses S5 to the primary"
        );

        assert!(
            slot.read_replica().reject_write().is_err(),
            "S5 is read-only (a write attempt errors)"
        );
    }

    #[test]
    fn stubbed_list_objects_errors_loudly() {
        let slot = FailClosedCheck::new();
        let at = Consistency {
            at_least: myelin_identity::Zookie("z".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        let r = slot.list_objects(
            &principal(),
            &Permission("read".into()),
            &ObjectType("issue".into()),
            &at,
        );
        assert!(
            matches!(r, Err(AuthzError::NotYetImplemented(_))),
            "list_objects errors loudly until P-ID-11/12 (never a permissive set)"
        );
    }

    #[test]
    fn identity_store_auto_registers_as_holder() {
        let handle = boot_identity(Config::default(), OutboxStore::new()).expect("boot");
        assert!(
            handle
                .holder_registry()
                .is_registered(myelin_substrate::StoreKind::Oltp, "identity"),
            "the identity OLTP store auto-registered as a PersonalDataHolder"
        );
    }

    #[test]
    fn identity_service_serves_and_drains_cleanly() {
        assert_eq!(
            serve(identity_app_spec(Config::default(), OutboxStore::new())),
            Ok(()),
            "the identity service boots → … → drains cleanly"
        );
    }

    #[test]
    fn identity_failed_boot_returns_non_zero() {
        let r = boot_identity(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed identity boot returns non-zero (Err)");
    }
}
