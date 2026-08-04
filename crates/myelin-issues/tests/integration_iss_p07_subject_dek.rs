#![cfg(feature = "integration")]

use myelin_events::{Actor, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_issues::{decrypt_free_text, is_raw_principal_id, IssueDraft};
use myelin_storage::kms::KmsEngine;
use myelin_tenancy::{ArtifactRef as IdArtifactRef, Region, TenantId};
use std::collections::HashMap;
use std::sync::Arc;

type IdResult<T> = myelin_identity::Result<T>;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn actor() -> Principal {
    Principal::stub(PrincipalId("u-42".into()), PrincipalKind::Human, tenant())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(actor()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: None,
    }
}

struct CdcId {
    pseudonyms: HashMap<String, String>,
}
impl IdentityService for CdcId {
    fn resolve_pseudonym(&self, subject: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        self.pseudonyms
            .get(&subject.0)
            .cloned()
            .ok_or(AuthzError::NotYetImplemented("no map entry"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &IdArtifactRef,
        _a: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(Decision::Allow)
    }
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _a: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _a: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Ok(Zookie("zk".into()))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

#[tokio::test]
async fn pseudonymous_columns_and_subject_dek_free_text_at_rest_on_real_postgres() {
    use sqlx::Row;

    let store = OutboxStore::new();
    let engine = KmsEngine::new();
    let id = CdcId {
        pseudonyms: HashMap::from([("u-42".to_string(), "8a2f@acme.noreply".to_string())]),
    };
    let plaintext_title = "fix the login bug for Ada Lovelace";
    let plaintext_props = b"{\"customer\":\"ada@example.com\"}".to_vec();
    let draft = IssueDraft {
        project_id: 7,
        title: plaintext_title.into(),
        props: plaintext_props.clone(),
        reporter_pseudonym: "u-42".into(),
    };
    let (_, sealed) = myelin_issues::apply_mutation_sealed(
        &store,
        Arc::new(MonotonicMinter::new()),
        ctx_base(),
        &id,
        &engine,
        &actor(),
        "ENG-1",
        &draft,
        None,
    )
    .expect("a sealed create commits");

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let tbl = format!("issue_p373_{suffix}");
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {tbl} (\
           issue_local_id TEXT PRIMARY KEY, \
           reporter_pseudonym TEXT NOT NULL, \
           title_ciphertext BYTEA NOT NULL, \
           title_pii_key_ref TEXT NOT NULL, \
           title_nonce BYTEA NOT NULL, \
           props_ciphertext BYTEA NOT NULL, \
           props_pii_key_ref TEXT NOT NULL, \
           props_nonce BYTEA NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create issue table");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant issue table");

    sqlx::query(&format!(
        "INSERT INTO {tbl} (issue_local_id, reporter_pseudonym, \
           title_ciphertext, title_pii_key_ref, title_nonce, \
           props_ciphertext, props_pii_key_ref, props_nonce) \
         VALUES ('ENG-1', $1, $2, $3, $4, $5, $6, $7)"
    ))
    .bind(sealed.reporter.render())
    .bind(&sealed.title.ciphertext)
    .bind(sealed.title.key_ref.to_uri())
    .bind(sealed.title.nonce.as_slice())
    .bind(&sealed.props.ciphertext)
    .bind(sealed.props.key_ref.to_uri())
    .bind(sealed.props.nonce.as_slice())
    .execute(&app)
    .await
    .expect("write the at-rest issue row");

    let row = sqlx::query(&format!(
        "SELECT reporter_pseudonym, title_ciphertext, props_ciphertext FROM {tbl} WHERE issue_local_id = 'ENG-1'"
    ))
    .fetch_one(&app)
    .await
    .expect("read the at-rest row back");

    let reporter_at_rest: String = row.get("reporter_pseudonym");
    let title_ct_at_rest: Vec<u8> = row.get("title_ciphertext");
    let props_ct_at_rest: Vec<u8> = row.get("props_ciphertext");

    assert_eq!(reporter_at_rest, "8a2f@acme.noreply");
    assert_ne!(
        reporter_at_rest, "u-42",
        "the raw principal id is never at rest"
    );
    assert!(
        !is_raw_principal_id(&reporter_at_rest),
        "the reporter column at rest is a pseudonym, not a raw id"
    );

    let contains = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
    assert!(
        !contains(&title_ct_at_rest, plaintext_title.as_bytes()),
        "0 plaintext title at rest in the real Postgres column"
    );
    assert!(
        !contains(&title_ct_at_rest, b"Ada Lovelace"),
        "the title PII byte-run is never at rest"
    );
    assert!(
        !contains(&props_ct_at_rest, b"ada@example.com"),
        "0 plaintext props PII at rest in the real Postgres column"
    );

    let region = Region::new("fr-par");
    let mut from_db_title = sealed.title.clone();
    from_db_title.ciphertext = title_ct_at_rest;
    let opened = decrypt_free_text(&engine, &region, &from_db_title)
        .expect("the ciphertext read from the DB decrypts while the key lives");
    assert_eq!(
        opened,
        plaintext_title.as_bytes(),
        "the free-text round-trips through the per-subject DEK"
    );

    let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await;
}
