#![cfg(feature = "integration")]

use myelin_events::{
    Actor, ArtifactRef, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, OUTBOX_MIGRATION,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy, FragmentAdmit,
    IdentityService, ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission,
    Precondition, Principal, PrincipalId, PrincipalKind, RewriteTrace, RunId, RunToken,
    SubjectTree, TupleDelta, Zookie,
};
use myelin_issues::{apply_mutation, IssueDraft, MutationKind, PERM_MANAGE};
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
    TenantId("tenantA".into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-7".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: None,
    }
}

struct AllowId;
impl IdentityService for AllowId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _a: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(Decision::Allow)
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
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn staged_issue_rows() -> Vec<(String, String, String, i64, serde_json::Value)> {
    let store = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>;
    let id = AllowId;
    apply_mutation(
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &id,
        &principal(),
        "ENG-1",
        &MutationKind::Create(IssueDraft {
            project_id: 7,
            title: "fix the charge bug".into(),
            props: b"{}".to_vec(),
            reporter_pseudonym: "psn:abc".into(),
        }),
        None,
    )
    .expect("create commits");
    apply_mutation(
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &id,
        &principal(),
        "ENG-1",
        &MutationKind::Create(IssueDraft {
            project_id: 7,
            title: "fix the charge bug (again)".into(),
            props: b"{}".to_vec(),
            reporter_pseudonym: "psn:abc".into(),
        }),
        None,
    )
    .expect("second event commits");

    store
        .committed_rows()
        .into_iter()
        .map(|row| {
            (
                row.event_id.0.clone(),
                row.aggregate.0.clone(),
                row.subject.0.clone(),
                row.seq as i64,
                serde_json::to_value(&row.envelope).expect("envelope → jsonb"),
            )
        })
        .collect()
}

#[tokio::test]
async fn issue_write_path_emit_iff_committed_on_real_postgres() {
    use sqlx::Row;

    assert_eq!(PERM_MANAGE, "manage");

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
    let outbox = format!("outbox_p372_{suffix}");
    let issue_tbl = format!("issue_p372_{suffix}");

    let outbox_ddl = OUTBOX_MIGRATION
        .replace("EXISTS outbox (", &format!("EXISTS {outbox} ("))
        .replace("ON outbox (", &format!("ON {outbox} ("))
        .replace(
            "outbox_event_id_unique",
            &format!("{outbox}_event_id_unique"),
        )
        .replace(
            "outbox_aggregate_seq_unique",
            &format!("{outbox}_aggregate_seq_unique"),
        )
        .replace("outbox_unsent_idx", &format!("{outbox}_unsent_idx"));
    for stmt in outbox_ddl
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(stmt)
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("outbox ddl `{stmt}`: {e}"));
    }
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {issue_tbl} (issue_local_id TEXT PRIMARY KEY, project_id BIGINT, title TEXT)"
    ))
    .execute(&admin)
    .await
    .expect("create issue table");
    sqlx::query(&format!("GRANT ALL ON {outbox} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant outbox");
    sqlx::query(&format!("GRANT ALL ON {issue_tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant issue");

    let rows = staged_issue_rows();
    assert_eq!(rows.len(), 2, "the write path emitted 2 issue.* events");

    let insert_outbox = format!(
        "INSERT INTO {outbox} (event_id, aggregate, seq, subject, envelope) VALUES ($1,$2,$3,$4,$5)"
    );

    let mut tx = app
        .begin()
        .await
        .expect("begin the issue state transaction");
    sqlx::query(&format!(
        "INSERT INTO {issue_tbl} (issue_local_id, project_id, title) VALUES ('ENG-1', 7, 'fix the charge bug')"
    ))
    .execute(&mut *tx)
    .await
    .expect("write the issue state row");
    for (event_id, aggregate, subject, seq, envelope) in rows.iter() {
        sqlx::query(&insert_outbox)
            .bind(event_id)
            .bind(aggregate)
            .bind(seq)
            .bind(subject)
            .bind(envelope)
            .execute(&mut *tx)
            .await
            .expect("emit the issue event into the SAME tx");
    }
    tx.commit()
        .await
        .expect("commit the issue + events together");

    let n: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} \
         WHERE published_at IS NULL AND envelope->>'type_' LIKE 'issue.%'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n, 2,
        "2 issue.* events committed → 2 relay-visible issue.* rows (0 lost)"
    );
    let c: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {issue_tbl}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("n");
    assert_eq!(c, 1, "the issue state row co-committed");

    let agg: String = sqlx::query(&format!("SELECT DISTINCT aggregate FROM {outbox}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("aggregate");
    assert_eq!(agg, "issue:7:ENG-1", "the aggregate is the ISSUE (§5)");
    let seqs: Vec<i64> = sqlx::query(&format!(
        "SELECT seq FROM {outbox} WHERE aggregate = 'issue:7:ENG-1' ORDER BY seq"
    ))
    .fetch_all(&app)
    .await
    .unwrap()
    .into_iter()
    .map(|r| r.get::<i64, _>("seq"))
    .collect();
    assert_eq!(
        seqs,
        vec![0, 1],
        "per-issue seq is monotonic + gap-free (0,1)"
    );

    let mut tx2 = app.begin().await.expect("begin a second state transaction");
    sqlx::query(&format!(
        "INSERT INTO {issue_tbl} (issue_local_id, project_id, title) VALUES ('ENG-2', 7, 'never commits')"
    ))
    .execute(&mut *tx2)
    .await
    .expect("write a second issue row");
    let rows2 = staged_issue_rows();
    for (i, (event_id, aggregate, subject, _seq, envelope)) in rows2.iter().enumerate() {
        sqlx::query(&insert_outbox)
            .bind(format!("{event_id}-tx2"))
            .bind(aggregate)
            .bind(100 + i as i64)
            .bind(subject)
            .bind(envelope)
            .execute(&mut *tx2)
            .await
            .expect("emit the second event set into the SAME tx");
    }
    tx2.rollback()
        .await
        .expect("ABORT the state transaction (the crash before commit)");

    let n_after: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} WHERE envelope->>'type_' LIKE 'issue.%'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n_after, 2,
        "aborted state tx wrote 0 events (emit-iff-committed): still only the 2 committed"
    );
    let c_after: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {issue_tbl}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        c_after, 1,
        "the aborted issue row rolled back too (no issue without its event)"
    );

    sqlx::query(&format!("DROP TABLE {outbox}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {issue_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
