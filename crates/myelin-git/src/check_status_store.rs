use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusConsumer, CheckStatusRow, GitOid, TrustTier,
};
use crate::merge_gate::{evaluate_merge_gate_row, MergeGateOutcome, MergeGatePolicy, UnmetContext};
use myelin_events::{
    consume, Backoff, Consumer, ConsumerName, ConsumerSpec, DedupLedger, EventEnvelope,
    EventHandler, HandleOutcome, Reason, SubjectPattern, SubscribeError,
};
use myelin_storage::{
    HotTables, Migration, MigrationPhase, Migrations, SubstrateProvider, TenantScope,
};
use sqlx::postgres::PgPool;
use sqlx::Row;

pub const CHECK_STATUS_TABLE: &str = "check_status";
pub const CHECK_STATUS_CONSUMER: &str = "git.check_status";
pub const CHECK_STATUS_SUBJECT_PREFIX: &str = "myelin://";

fn check_status_subjects() -> &'static [SubjectPattern] {
    static SUBJECTS: std::sync::OnceLock<Vec<SubjectPattern>> = std::sync::OnceLock::new();
    SUBJECTS
        .get_or_init(|| vec![SubjectPattern(CHECK_STATUS_SUBJECT_PREFIX.to_string())])
        .as_slice()
}

pub const CREATE_CHECK_STATUS_DDL: &str = r#"
CREATE TABLE check_status (
  tenant_id text NOT NULL,
  region text NOT NULL,
  repo_ref text NOT NULL,
  commit_oid text NOT NULL,
  context_provider text NOT NULL CHECK (context_provider IN ('ci', 'external')),
  context_name text NOT NULL,
  state text NOT NULL CHECK (
    state IN ('queued','in_progress','success','failure','error','neutral','cancelled')
  ),
  required boolean NOT NULL,
  run_ref text NOT NULL,
  run_attempt bigint NOT NULL CHECK (run_attempt BETWEEN 0 AND 4294967295),
  trust_tier text NOT NULL CHECK (trust_tier IN ('trusted','untrusted_fork')),
  details_ref text NOT NULL,
  summary_key text NOT NULL,
  summary_args jsonb NOT NULL CHECK (jsonb_typeof(summary_args) = 'object'),
  cost_settled boolean NOT NULL,
  started_at text NOT NULL,
  completed_at text,
  PRIMARY KEY (
    tenant_id, region, repo_ref, commit_oid, context_provider, context_name
  )
);
SELECT myelin_make_tenant_scoped('check_status');
"#;

pub const CREATE_CHECK_STATUS_COMMIT_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS check_status_commit_idx
  ON check_status (tenant_id, region, repo_ref, commit_oid)
"#;

pub fn check_status_migrations() -> Migrations {
    Migrations::of([
        Migration::plain_on(
            "git_0014_check_status",
            CREATE_CHECK_STATUS_DDL,
            CHECK_STATUS_TABLE,
        ),
        Migration::phased(
            "git_0015_check_status_commit_index",
            CREATE_CHECK_STATUS_COMMIT_INDEX_DDL,
            MigrationPhase::Expand,
            CHECK_STATUS_TABLE,
        ),
    ])
}

pub fn check_status_hot_tables() -> HotTables {
    HotTables::declare([CHECK_STATUS_TABLE])
}

pub fn projection_ddl(table: &str, dedup_table: &str) -> Result<String, sqlx::Error> {
    validate_sql_identifier("projection table", table)?;
    validate_sql_identifier("dedup table", dedup_table)?;
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {table} (\
            tenant_id         text   NOT NULL,\
            region            text   NOT NULL,\
            repo_ref          text   NOT NULL,\
            commit_oid        text   NOT NULL,\
            context_provider  text   NOT NULL CHECK (context_provider IN ('ci', 'external')),\
            context_name      text   NOT NULL,\
            state             text   NOT NULL CHECK (state IN ('queued', 'in_progress', 'success',\
                                      'failure', 'error', 'neutral', 'cancelled')),\
            required          boolean NOT NULL,\
            run_ref           text   NOT NULL,\
            run_attempt       bigint NOT NULL CHECK (run_attempt BETWEEN 0 AND 4294967295),\
            trust_tier        text   NOT NULL CHECK (trust_tier IN ('trusted', 'untrusted_fork')),\
            details_ref       text   NOT NULL,\
            summary_key       text   NOT NULL,\
            summary_args      jsonb  NOT NULL CHECK (jsonb_typeof(summary_args) = 'object'),\
            cost_settled      boolean NOT NULL,\
            started_at        text NOT NULL,\
            completed_at      text,\
            PRIMARY KEY (tenant_id, region, repo_ref, commit_oid, context_provider, context_name));\
         CREATE TABLE IF NOT EXISTS {dedup_table} (\
            consumer  text NOT NULL,\
            event_id  text NOT NULL,\
            recorded_at timestamptz NOT NULL DEFAULT now(),\
            CONSTRAINT {dedup_table}_pk PRIMARY KEY (consumer, event_id))"
    ))
}

fn validate_sql_identifier(kind: &str, identifier: &str) -> Result<(), sqlx::Error> {
    let mut chars = identifier.chars();
    let starts_safely = chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    let remainder_is_safe =
        chars.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if starts_safely && remainder_is_safe && identifier.len() <= 63 {
        return Ok(());
    }
    Err(sqlx::Error::Protocol(format!(
        "invalid {kind} identifier {identifier:?}: expected 1..=63 ASCII bytes matching [A-Za-z_][A-Za-z0-9_]*"
    )))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreApplyOutcome {
    Superseded,
    DroppedStale,
    DuplicateEvent,
}

#[derive(Clone)]
pub struct PgCheckStatusProjection {
    pool: PgPool,
    table: String,
    dedup_table: String,
    consumer: String,
    provider: Option<SubstrateProvider>,
    admission_provider: Option<SubstrateProvider>,
    runtime: tokio::runtime::Handle,
}

impl PgCheckStatusProjection {
    pub fn production(
        provider: SubstrateProvider,
        admission_provider: SubstrateProvider,
        runtime: tokio::runtime::Handle,
    ) -> PgCheckStatusProjection {
        PgCheckStatusProjection {
            pool: provider.db_pool().clone(),
            table: CHECK_STATUS_TABLE.to_string(),
            dedup_table: "consumer_dedup".to_string(),
            consumer: CHECK_STATUS_CONSUMER.to_string(),
            provider: Some(provider),
            admission_provider: Some(admission_provider),
            runtime,
        }
    }

    pub async fn connect(
        pool: PgPool,
        table: &str,
        dedup_table: &str,
        consumer: &str,
    ) -> Result<PgCheckStatusProjection, sqlx::Error> {
        let ddl = projection_ddl(table, dedup_table)?;
        myelin_storage::with_migration_lock(&pool, &ddl)
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("check_status migration: {e}")))?;
        Ok(PgCheckStatusProjection {
            pool,
            table: table.to_string(),
            dedup_table: dedup_table.to_string(),
            consumer: consumer.to_string(),
            provider: None,
            admission_provider: None,
            runtime: tokio::runtime::Handle::current(),
        })
    }

    pub async fn apply(
        &self,
        event_id: &str,
        region: &str,
        fact: &CheckStatus,
    ) -> Result<StoreApplyOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let tenant_id = &fact.tenant.0;

        // @tenant-cross-scope: consumer_dedup is consumer-internal event identity, not tenant data.
        let dedup = sqlx::query(&format!(
            "INSERT INTO {} (consumer, event_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
            self.dedup_table
        ))
        .bind(&self.consumer)
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
        if dedup.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(StoreApplyOutcome::DuplicateEvent);
        }

        lock_check_admission(
            &mut tx,
            &fact.tenant.0,
            region,
            &fact.repo.0,
            &fact.commit_oid.0,
        )
        .await?;

        let provider = match fact.context.provider {
            crate::check_status::CheckProvider::Ci => "ci",
            crate::check_status::CheckProvider::External => "external",
        };
        let state = state_str(fact.state);
        let trust = trust_str(fact.trust_tier);
        let summary_args = serde_json::to_value(&fact.summary.args)
            .expect("BTreeMap<String,String> always serialises to a JSON object");

        let upsert = sqlx::query(&format!(
            "INSERT INTO {table} (tenant_id, region, repo_ref, commit_oid, context_provider, \
                context_name, state, required, run_ref, run_attempt, trust_tier, details_ref, \
                summary_key, summary_args, cost_settled, started_at, completed_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) \
             ON CONFLICT (tenant_id, region, repo_ref, commit_oid, context_provider, context_name) \
             DO UPDATE SET \
               state = EXCLUDED.state, run_ref = EXCLUDED.run_ref, run_attempt = EXCLUDED.run_attempt, \
               required = EXCLUDED.required, \
               trust_tier = EXCLUDED.trust_tier, details_ref = EXCLUDED.details_ref, \
               summary_key = EXCLUDED.summary_key, summary_args = EXCLUDED.summary_args, \
               cost_settled = EXCLUDED.cost_settled, started_at = EXCLUDED.started_at, \
               completed_at = EXCLUDED.completed_at \
             WHERE EXCLUDED.run_attempt >= {table}.run_attempt",
            table = self.table
        ))
        .bind(tenant_id)
        .bind(region)
        .bind(&fact.repo.0)
        .bind(&fact.commit_oid.0)
        .bind(provider)
        .bind(&fact.context.name)
        .bind(state)
        .bind(fact.required)
        .bind(&fact.run.0)
        .bind(i64::from(fact.run_attempt))
        .bind(trust)
        .bind(&fact.details_ref.0)
        .bind(&fact.summary.template_key)
        .bind(&summary_args)
        .bind(fact.cost_settled)
        .bind(&fact.started_at.0)
        .bind(fact.completed_at.as_ref().map(|timestamp| &timestamp.0))
        .execute(&mut *tx)
        .await?;

        let outcome = if upsert.rows_affected() == 0 {
            StoreApplyOutcome::DroppedStale
        } else {
            StoreApplyOutcome::Superseded
        };
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn apply_in_tx(
        conn: &mut sqlx::PgConnection,
        region: &str,
        fact: &CheckStatus,
    ) -> Result<StoreApplyOutcome, sqlx::Error> {
        apply_projection_in_tx(CHECK_STATUS_TABLE, conn, region, fact).await
    }

    pub async fn current(
        &self,
        tenant_id: &str,
        region: &str,
        repo_ref: &str,
        commit_oid: &GitOid,
        provider: &str,
        context_name: &str,
    ) -> Result<Option<CheckStatusRow>, sqlx::Error> {
        let row = sqlx::query(&format!(
            "SELECT tenant_id, commit_oid, context_provider, context_name, state, run_ref, \
                    run_attempt, trust_tier, details_ref, summary_key, summary_args, cost_settled \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND repo_ref = $3 AND commit_oid = $4 \
                       AND context_provider = $5 AND context_name = $6",
            self.table
        ))
        .bind(tenant_id)
        .bind(region)
        .bind(repo_ref)
        .bind(&commit_oid.0)
        .bind(provider)
        .bind(context_name)
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_row).transpose()
    }

    pub async fn merge_gate(
        &self,
        tenant_id: &str,
        region: &str,
        repo_ref: &str,
        head_oid: &GitOid,
        policy: &MergeGatePolicy,
        endorsed_contexts: &[CheckContext],
    ) -> Result<MergeGateOutcome, sqlx::Error> {
        let mut unmet: Vec<UnmetContext> = Vec::new();
        for ctx in &policy.required {
            let provider = match ctx.provider {
                crate::check_status::CheckProvider::Ci => "ci",
                crate::check_status::CheckProvider::External => "external",
            };
            let row = self
                .current(tenant_id, region, repo_ref, head_oid, provider, &ctx.name)
                .await?;
            let endorsed = endorsed_contexts.contains(ctx);
            if let Some(reason) = evaluate_merge_gate_row(row.as_ref(), endorsed) {
                unmet.push(UnmetContext {
                    context: ctx.clone(),
                    reason,
                });
            }
        }
        Ok(if unmet.is_empty() {
            MergeGateOutcome::Admitted
        } else {
            MergeGateOutcome::Blocked { unmet }
        })
    }

    pub async fn row_count_for_commit(
        &self,
        tenant_id: &str,
        region: &str,
        repo_ref: &str,
        commit_oid: &GitOid,
    ) -> Result<i64, sqlx::Error> {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {} WHERE tenant_id = $1 AND region = $2 AND repo_ref = $3 \
             AND commit_oid = $4",
            self.table
        ))
        .bind(tenant_id)
        .bind(region)
        .bind(repo_ref)
        .bind(&commit_oid.0)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    pub async fn drop_tables(&self) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS {}; DROP TABLE IF EXISTS {}",
            self.table, self.dedup_table
        ))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub fn rows_for_commit(
        &self,
        scope: &TenantScope,
        repo_ref: &str,
        commit_oid: &GitOid,
    ) -> Result<Vec<CheckStatusRow>, sqlx::Error> {
        let mut rows = self.rows_for_commits(scope, repo_ref, std::slice::from_ref(commit_oid))?;
        Ok(rows.remove(&commit_oid.0).unwrap_or_default())
    }

    pub fn rows_for_commits(
        &self,
        scope: &TenantScope,
        repo_ref: &str,
        commit_oids: &[GitOid],
    ) -> Result<std::collections::BTreeMap<String, Vec<CheckStatusRow>>, sqlx::Error> {
        if commit_oids.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }
        let provider = self.provider.clone().ok_or_else(|| {
            sqlx::Error::Protocol("production projection provider is unavailable".into())
        })?;
        if scope.region().0 != provider.config().region {
            return Err(sqlx::Error::Protocol(
                "check_status scope is outside the configured region".into(),
            ));
        }
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let repo_ref = repo_ref.to_string();
        let commit_oids = commit_oids
            .iter()
            .map(|oid| oid.0.clone())
            .collect::<Vec<_>>();
        let table = self.table.clone();
        tokio::task::block_in_place(|| {
            self.runtime.block_on(async move {
                provider
                    .with_tenant_tx(&tenant.clone(), move |conn| {
                        Box::pin(async move {
                            let rows = sqlx::query(&format!(
                                "SELECT tenant_id,commit_oid,context_provider,context_name,state,\
                                        run_ref,run_attempt,trust_tier,details_ref,summary_key,\
                                        summary_args,cost_settled \
                                   FROM {table} \
                                  WHERE tenant_id=$1 AND region=$2 AND repo_ref=$3 \
                                    AND commit_oid = ANY($4) \
                                  ORDER BY commit_oid,context_provider,context_name"
                            ))
                            .bind(&tenant)
                            .bind(&region)
                            .bind(&repo_ref)
                            .bind(&commit_oids)
                            .fetch_all(&mut *conn)
                            .await
                            .map_err(|_| {
                                myelin_storage::PgError::Query(
                                    "read Git check projection failed".into(),
                                )
                            })?;
                            let mut by_commit = std::collections::BTreeMap::new();
                            for row in rows {
                                let row = decode_row(row).map_err(|_| {
                                    myelin_storage::PgError::Query(
                                        "Git check projection row is malformed".into(),
                                    )
                                })?;
                                by_commit
                                    .entry(row.commit_oid.0.clone())
                                    .or_insert_with(Vec::new)
                                    .push(row);
                            }
                            Ok(by_commit)
                        })
                    })
                    .await
            })
        })
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))
    }

    pub fn with_admission_snapshot<R, F>(
        &self,
        scope: &TenantScope,
        repo_ref: &str,
        commit_oids: &[GitOid],
        op: F,
    ) -> Result<R, sqlx::Error>
    where
        R: Send,
        F: FnOnce(std::collections::BTreeMap<String, Vec<CheckStatusRow>>) -> R + Send + 'static,
    {
        let provider = self.admission_provider.clone().ok_or_else(|| {
            sqlx::Error::Protocol("protected-push admission lane is unavailable".into())
        })?;
        if scope.region().0 != provider.config().region {
            return Err(sqlx::Error::Protocol(
                "check_status scope is outside the configured region".into(),
            ));
        }
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let repo_ref = repo_ref.to_string();
        let mut commit_oids = commit_oids
            .iter()
            .map(|oid| oid.0.clone())
            .collect::<Vec<_>>();
        commit_oids.sort();
        commit_oids.dedup();
        tokio::task::block_in_place(|| {
            self.runtime.block_on(async move {
                provider
                    .with_tenant_tx(&tenant.clone(), move |conn| {
                        Box::pin(async move {
                            let mut by_commit = std::collections::BTreeMap::new();
                            for commit_oid in &commit_oids {
                                lock_check_admission(conn, &tenant, &region, &repo_ref, commit_oid)
                                    .await
                                    .map_err(|_| {
                                        myelin_storage::PgError::Query(
                                            "lock protected-push check admission failed".into(),
                                        )
                                    })?;
                                let rows = rows_for_commit_in_tx(
                                    conn, &tenant, &region, &repo_ref, commit_oid,
                                )
                                .await
                                .map_err(|_| {
                                    myelin_storage::PgError::Query(
                                        "read protected-push check projection failed".into(),
                                    )
                                })?;
                                by_commit.insert(commit_oid.clone(), rows);
                            }
                            Ok(op(by_commit))
                        })
                    })
                    .await
            })
        })
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))
    }
}

async fn apply_projection_in_tx(
    table: &str,
    conn: &mut sqlx::PgConnection,
    region: &str,
    fact: &CheckStatus,
) -> Result<StoreApplyOutcome, sqlx::Error> {
    lock_check_admission(
        conn,
        &fact.tenant.0,
        region,
        &fact.repo.0,
        &fact.commit_oid.0,
    )
    .await?;
    let provider = match fact.context.provider {
        crate::check_status::CheckProvider::Ci => "ci",
        crate::check_status::CheckProvider::External => "external",
    };
    let summary_args = serde_json::to_value(&fact.summary.args)
        .expect("BTreeMap<String,String> always serialises to a JSON object");
    let upsert = sqlx::query(&format!(
        "INSERT INTO {table} (tenant_id,region,repo_ref,commit_oid,context_provider,context_name,\
             state,required,run_ref,run_attempt,trust_tier,details_ref,summary_key,summary_args,\
             cost_settled,started_at,completed_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) \
         ON CONFLICT (tenant_id,region,repo_ref,commit_oid,context_provider,context_name) \
         DO UPDATE SET state=EXCLUDED.state,required=EXCLUDED.required,run_ref=EXCLUDED.run_ref,\
             run_attempt=EXCLUDED.run_attempt,trust_tier=EXCLUDED.trust_tier,\
             details_ref=EXCLUDED.details_ref,summary_key=EXCLUDED.summary_key,\
             summary_args=EXCLUDED.summary_args,cost_settled=EXCLUDED.cost_settled,\
             started_at=EXCLUDED.started_at,completed_at=EXCLUDED.completed_at \
         WHERE EXCLUDED.run_attempt >= {table}.run_attempt"
    ))
    .bind(&fact.tenant.0)
    .bind(region)
    .bind(&fact.repo.0)
    .bind(&fact.commit_oid.0)
    .bind(provider)
    .bind(&fact.context.name)
    .bind(state_str(fact.state))
    .bind(fact.required)
    .bind(&fact.run.0)
    .bind(i64::from(fact.run_attempt))
    .bind(trust_str(fact.trust_tier))
    .bind(&fact.details_ref.0)
    .bind(&fact.summary.template_key)
    .bind(&summary_args)
    .bind(fact.cost_settled)
    .bind(&fact.started_at.0)
    .bind(fact.completed_at.as_ref().map(|timestamp| &timestamp.0))
    .execute(conn)
    .await?;
    Ok(if upsert.rows_affected() == 0 {
        StoreApplyOutcome::DroppedStale
    } else {
        StoreApplyOutcome::Superseded
    })
}

pub(crate) async fn lock_check_admission(
    conn: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    repo_ref: &str,
    commit_oid: &str,
) -> Result<(), sqlx::Error> {
    let key = format!(
        "myelin.check-admission.v1|{}:{tenant_id}|{}:{region}|{}:{repo_ref}|{}:{commit_oid}",
        tenant_id.len(),
        region.len(),
        repo_ref.len(),
        commit_oid.len(),
    );
    // @tenant-cross-scope: advisory lock state reads no tenant rows; its injective key already
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(conn)
        .await?;
    Ok(())
}

pub(crate) async fn rows_for_commit_in_tx(
    conn: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    repo_ref: &str,
    commit_oid: &str,
) -> Result<Vec<CheckStatusRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT tenant_id,commit_oid,context_provider,context_name,state,run_ref,run_attempt,\
                trust_tier,details_ref,summary_key,summary_args,cost_settled \
           FROM check_status \
          WHERE tenant_id=$1 AND region=$2 AND repo_ref=$3 AND commit_oid=$4 \
          ORDER BY context_provider,context_name",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(repo_ref)
    .bind(commit_oid)
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(decode_row).collect()
}

pub struct DurableCheckStatusConsumer {
    runtime: tokio::runtime::Handle,
    expected_region: String,
}

impl DurableCheckStatusConsumer {
    pub fn new(runtime: tokio::runtime::Handle, expected_region: impl Into<String>) -> Self {
        Self {
            runtime,
            expected_region: expected_region.into(),
        }
    }
}

impl EventHandler for DurableCheckStatusConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        check_status_subjects()
    }

    fn handle(
        &self,
        event: &EventEnvelope,
        tx: &mut myelin_events::HandlerTx<'_>,
    ) -> HandleOutcome {
        if event.type_.0 != myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED {
            return HandleOutcome::NonRetryable(Reason(
                "Git check consumer received a non-ci.check.updated event".into(),
            ));
        }
        if event.region.0 != self.expected_region {
            return HandleOutcome::NonRetryable(Reason(
                "Git check event is outside the consumer region".into(),
            ));
        }
        let fact = match CheckStatusConsumer::decode(&event.payload) {
            Ok(fact) => fact,
            Err(reason) => return HandleOutcome::NonRetryable(reason),
        };
        if let Err(reason) = validate_fact_provenance(event, &fact) {
            return HandleOutcome::NonRetryable(reason);
        }
        let Some(conn) = tx.connection::<sqlx::PgConnection>() else {
            return HandleOutcome::Retry(Backoff { seconds: 2 });
        };
        let region = event.region.0.clone();
        match tokio::task::block_in_place(|| {
            self.runtime
                .block_on(PgCheckStatusProjection::apply_in_tx(conn, &region, &fact))
        }) {
            Ok(_) => HandleOutcome::Done,
            Err(_) => HandleOutcome::Retry(Backoff { seconds: 2 }),
        }
    }
}

fn validate_fact_provenance(event: &EventEnvelope, fact: &CheckStatus) -> Result<(), Reason> {
    if fact.tenant != event.tenant {
        return Err(Reason(
            "CheckStatus tenant does not match its event envelope".into(),
        ));
    }
    let prefix = format!("myelin://{}/git/repo/", event.tenant.0);
    let slug =
        fact.repo.0.strip_prefix(&prefix).ok_or_else(|| {
            Reason("CheckStatus repo is not canonical for its event tenant".into())
        })?;
    if slug.is_empty() || slug.contains(['#', '?']) {
        return Err(Reason(
            "CheckStatus repository slug is not canonical".into(),
        ));
    }
    crate::gix_backend::validate_repo_slug(slug)
        .map_err(|_| Reason("CheckStatus repository slug is not canonical".into()))?;
    if fact.context.provider != crate::check_status::CheckProvider::Ci {
        return Err(Reason(
            "ci.check.updated may only populate the CI check-provider namespace".into(),
        ));
    }
    let run = myelin_refs::parse_scoped(&fact.run.0)
        .map_err(|_| Reason("CheckStatus run is not a canonical ArtifactRef".into()))?;
    let details = myelin_refs::parse_scoped(&fact.details_ref.0)
        .map_err(|_| Reason("CheckStatus details_ref is not a canonical ArtifactRef".into()))?;
    if run.artifact_ref.0 != fact.run.0
        || details.artifact_ref.0 != fact.details_ref.0
        || run.tenant.0 != event.tenant.0
        || run.subsystem != "ci"
        || run.type_ != "run"
        || run.sub.is_some()
        || details.tenant != run.tenant
        || details.subsystem != run.subsystem
        || details.type_ != run.type_
        || details.id != run.id
        || !matches!(details.sub, None | Some(myelin_refs::Sub::Step(_)))
    {
        return Err(Reason(
            "CheckStatus run/details_ref do not name the same tenant-owned CI run".into(),
        ));
    }
    let expected_subject = myelin_events::check_seam::check_subject(
        &fact.repo.0,
        &fact.commit_oid.0,
        &fact.context.name,
    );
    let expected_aggregate =
        myelin_events::check_seam::check_aggregate(&fact.repo.0, &fact.commit_oid.0);
    if event.subject != expected_subject || event.aggregate != expected_aggregate {
        return Err(Reason(
            "CheckStatus subject/aggregate provenance does not match its payload".into(),
        ));
    }
    Ok(())
}

pub fn build_durable_check_consumer(
    runtime: tokio::runtime::Handle,
    expected_region: impl Into<String>,
    dedup: DedupLedger,
    dead_letters: std::sync::Arc<dyn myelin_events::DurableDeadLetter>,
) -> Result<Consumer<DurableCheckStatusConsumer>, SubscribeError> {
    consume(
        ConsumerSpec::new(
            ConsumerName(CHECK_STATUS_CONSUMER.into()),
            &[CHECK_STATUS_SUBJECT_PREFIX],
        ),
        DurableCheckStatusConsumer::new(runtime, expected_region),
        dedup,
    )
    .map(|consumer| {
        consumer.with_dead_letter_sink(myelin_events::DeadLetterSink::durable(dead_letters))
    })
}

fn state_str(state: CheckState) -> &'static str {
    match state {
        CheckState::Queued => "queued",
        CheckState::InProgress => "in_progress",
        CheckState::Success => "success",
        CheckState::Failure => "failure",
        CheckState::Error => "error",
        CheckState::Neutral => "neutral",
        CheckState::Cancelled => "cancelled",
    }
}

fn parse_state(s: &str) -> Result<CheckState, sqlx::Error> {
    match s {
        "queued" => Ok(CheckState::Queued),
        "in_progress" => Ok(CheckState::InProgress),
        "success" => Ok(CheckState::Success),
        "failure" => Ok(CheckState::Failure),
        "error" => Ok(CheckState::Error),
        "neutral" => Ok(CheckState::Neutral),
        "cancelled" => Ok(CheckState::Cancelled),
        other => Err(corrupt_projection(format!("unknown check state {other:?}"))),
    }
}

fn trust_str(tier: TrustTier) -> &'static str {
    match tier {
        TrustTier::Trusted => "trusted",
        TrustTier::UntrustedFork => "untrusted_fork",
    }
}

fn parse_trust(s: &str) -> Result<TrustTier, sqlx::Error> {
    match s {
        "trusted" => Ok(TrustTier::Trusted),
        "untrusted_fork" => Ok(TrustTier::UntrustedFork),
        other => Err(corrupt_projection(format!("unknown trust tier {other:?}"))),
    }
}

fn decode_row(row: sqlx::postgres::PgRow) -> Result<CheckStatusRow, sqlx::Error> {
    use crate::check_status::{CheckContext, CheckProvider, HumanisedRef};
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::collections::BTreeMap;

    let provider_raw = row.try_get::<String, _>("context_provider")?;
    let provider = match provider_raw.as_str() {
        "ci" => Ok(CheckProvider::Ci),
        "external" => Ok(CheckProvider::External),
        other => Err(corrupt_projection(format!(
            "unknown context provider {other:?}"
        ))),
    };
    let provider = provider?;
    let summary_args: BTreeMap<String, String> =
        serde_json::from_value(row.try_get::<serde_json::Value, _>("summary_args")?)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    let run_attempt = u32::try_from(row.try_get::<i64, _>("run_attempt")?)
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    Ok(CheckStatusRow {
        tenant: TenantId(row.try_get::<String, _>("tenant_id")?),
        commit_oid: GitOid(row.try_get::<String, _>("commit_oid")?),
        context: CheckContext {
            provider,
            name: row.try_get::<String, _>("context_name")?,
        },
        state: parse_state(&row.try_get::<String, _>("state")?)?,
        run: ArtifactRef(row.try_get::<String, _>("run_ref")?),
        run_attempt,
        trust_tier: parse_trust(&row.try_get::<String, _>("trust_tier")?)?,
        details_ref: ArtifactRef(row.try_get::<String, _>("details_ref")?),
        summary: HumanisedRef {
            template_key: row.try_get::<String, _>("summary_key")?,
            args: summary_args,
        },
        cost_settled: row.try_get::<bool, _>("cost_settled")?,
    })
}

fn corrupt_projection(detail: String) -> sqlx::Error {
    sqlx::Error::Protocol(format!("corrupt check_status projection: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::{parse_state, parse_trust, projection_ddl};

    #[test]
    fn projection_identifiers_are_allowlisted_before_ddl_interpolation() {
        let ddl = projection_ddl("check_status_42", "consumer_dedup_42")
            .expect("ordinary unquoted identifiers are accepted");
        assert!(ddl.contains("CREATE TABLE IF NOT EXISTS check_status_42"));

        for unsafe_name in [
            "",
            "9status",
            "status; DROP TABLE users",
            "status-name",
            "état",
        ] {
            let error = projection_ddl(unsafe_name, "consumer_dedup")
                .expect_err("unsafe projection identifier must be refused");
            assert!(error
                .to_string()
                .contains("invalid projection table identifier"));
        }
        let overlong = "a".repeat(64);
        assert!(projection_ddl("check_status", &overlong).is_err());
    }

    #[test]
    fn corrupt_closed_set_values_return_errors_instead_of_panicking() {
        assert!(parse_state("future_state").is_err());
        assert!(parse_trust("root").is_err());
    }
}
