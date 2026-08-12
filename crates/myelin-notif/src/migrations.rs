use myelin_substrate::{Migration, MigrationPhase, Migrations};

pub const TABLES: [&str; 9] = [
    "notif_inbox_item",
    "notif_pref",
    "notif_quiet_hours",
    "notif_delivery",
    "notif_oncall_schedule",
    "notif_escalation_policy",
    "notif_escalation_run",
    "notif_humanise_template",
    "notif_mute",
];

pub const INBOX_ITEM_DDL: &str = "\
CREATE TABLE notif_inbox_item (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  item_id text NOT NULL, \
  recipient text NOT NULL, \
  subject text NOT NULL, \
  subject_root text NOT NULL, \
  reason text NOT NULL CHECK (reason IN ('approval_requested','escalated','sla','review_requested','assigned','mentioned','replied','agent_proposal','watched','state_changed','fyi','blocked','unblocked','thread_watched','shared','comments')), \
  class text NOT NULL CHECK (class IN ('critical','direct','participating','watching','fyi')), \
  origin_event text NOT NULL, \
  template_key text NOT NULL, \
  template_args_json jsonb NOT NULL, \
  dedup_key text NOT NULL, \
  coalesce_count integer NOT NULL DEFAULT 1, \
  state text NOT NULL DEFAULT 'unread' CHECK (state IN ('unread','seen','read','snoozed','archived','done')), \
  snooze_until timestamptz, \
  occurred_at timestamptz NOT NULL, \
  created_at timestamptz NOT NULL DEFAULT now(), \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, recipient, item_id), \
  UNIQUE (tenant_id, recipient, dedup_key))";

pub const INBOX_PRIORITY_CASE_SQL: &str = "CASE reason \
WHEN 'approval_requested' THEN 90 WHEN 'escalated' THEN 90 WHEN 'sla' THEN 90 \
WHEN 'review_requested' THEN 70 WHEN 'assigned' THEN 70 WHEN 'mentioned' THEN 70 \
WHEN 'shared' THEN 70 WHEN 'replied' THEN 55 WHEN 'agent_proposal' THEN 55 \
WHEN 'comments' THEN 55 WHEN 'watched' THEN 35 WHEN 'state_changed' THEN 35 \
WHEN 'thread_watched' THEN 35 WHEN 'blocked' THEN 35 WHEN 'unblocked' THEN 35 \
WHEN 'fyi' THEN 15 ELSE NULL END";

pub const INBOX_KEYSET_INDEX_MIGRATION_ID: &str = "notif_0010_inbox_recipient_keyset";
pub const INBOX_KEYSET_INDEX_DDL: &str = "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
notif_inbox_recipient_keyset ON notif_inbox_item \
(tenant_id, region, recipient, \
(CASE reason \
WHEN 'approval_requested' THEN 90 WHEN 'escalated' THEN 90 WHEN 'sla' THEN 90 \
WHEN 'review_requested' THEN 70 WHEN 'assigned' THEN 70 WHEN 'mentioned' THEN 70 \
WHEN 'shared' THEN 70 WHEN 'replied' THEN 55 WHEN 'agent_proposal' THEN 55 \
WHEN 'comments' THEN 55 WHEN 'watched' THEN 35 WHEN 'state_changed' THEN 35 \
WHEN 'thread_watched' THEN 35 WHEN 'blocked' THEN 35 WHEN 'unblocked' THEN 35 \
WHEN 'fyi' THEN 15 ELSE NULL END) DESC, item_id ASC)";

pub const INBOX_RECENCY_KEYSET_INDEX_MIGRATION_ID: &str =
    "notif_0011_inbox_recipient_recency_keyset";
pub const INBOX_RECENCY_KEYSET_INDEX_DDL: &str = "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
notif_inbox_recipient_recency_keyset ON notif_inbox_item \
(tenant_id, region, recipient, \
(CASE reason \
WHEN 'approval_requested' THEN 90 WHEN 'escalated' THEN 90 WHEN 'sla' THEN 90 \
WHEN 'review_requested' THEN 70 WHEN 'assigned' THEN 70 WHEN 'mentioned' THEN 70 \
WHEN 'shared' THEN 70 WHEN 'replied' THEN 55 WHEN 'agent_proposal' THEN 55 \
WHEN 'comments' THEN 55 WHEN 'watched' THEN 35 WHEN 'state_changed' THEN 35 \
WHEN 'thread_watched' THEN 35 WHEN 'blocked' THEN 35 WHEN 'unblocked' THEN 35 \
WHEN 'fyi' THEN 15 ELSE NULL END) DESC, occurred_at DESC, item_id ASC)";

pub const NOTIF_PREF_DDL: &str = "\
CREATE TABLE notif_pref (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  principal text NOT NULL, \
  routing jsonb NOT NULL, \
  digest jsonb, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, principal))";

pub const QUIET_HOURS_DDL: &str = "\
CREATE TABLE notif_quiet_hours (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  principal text NOT NULL, \
  tz text NOT NULL, \
  windows jsonb NOT NULL, \
  dnd_until timestamptz, \
  pierce_classes text[] NOT NULL DEFAULT '{critical}', \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, principal))";

pub const DELIVERY_DDL: &str = "\
CREATE TABLE notif_delivery (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  delivery_id text NOT NULL, \
  item_id text NOT NULL, \
  recipient text NOT NULL, \
  channel text NOT NULL CHECK (channel IN ('in_app','web_push','mobile_push','email','desktop')), \
  adapter text NOT NULL, \
  idem_key text NOT NULL, \
  state text NOT NULL CHECK (state IN ('pending','sent','delivered','bounced','failed','suppressed')), \
  attempts integer NOT NULL DEFAULT 0, \
  provider_ref text, \
  redacted boolean NOT NULL DEFAULT false, \
  created_at timestamptz NOT NULL DEFAULT now(), \
  sent_at timestamptz, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, delivery_id), \
  UNIQUE (tenant_id, idem_key))";

pub const ONCALL_SCHEDULE_DDL: &str = "\
CREATE TABLE notif_oncall_schedule (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  schedule_id text NOT NULL, \
  schedule_name text NOT NULL, \
  rotation jsonb NOT NULL, \
  tz text NOT NULL, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, schedule_id))";

pub const ESCALATION_POLICY_DDL: &str = "\
CREATE TABLE notif_escalation_policy (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  policy_id text NOT NULL, \
  policy_name text NOT NULL, \
  steps jsonb NOT NULL, \
  repeat integer NOT NULL DEFAULT 1, \
  ack_window interval NOT NULL, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, policy_id))";

pub const ESCALATION_RUN_DDL: &str = "\
CREATE TABLE notif_escalation_run (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  run_id text NOT NULL, \
  policy_id text NOT NULL, \
  trigger_event text NOT NULL, \
  workflow_ref text NOT NULL, \
  current_step integer NOT NULL DEFAULT 0, \
  state text NOT NULL CHECK (state IN ('active','acked','resolved','exhausted')), \
  acked_by text, \
  acked_at timestamptz, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, run_id))";

pub const HUMANISE_TEMPLATE_DDL: &str = "\
CREATE TABLE notif_humanise_template (\
  tenant_id text, \
  region text NOT NULL, \
  tenant_scope text GENERATED ALWAYS AS (COALESCE(tenant_id, '00000000-0000-0000-0000-000000000000')) STORED, \
  template_key text NOT NULL, \
  locale text NOT NULL DEFAULT 'en', \
  template_body text NOT NULL, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_scope, region, template_key, locale))";

pub const MUTE_DDL: &str = "\
CREATE TABLE notif_mute (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  principal text NOT NULL, \
  subject_root text NOT NULL, \
  until timestamptz, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, principal, subject_root))";

const TABLE_DDLS: &[(&str, &str, &str)] = &[
    ("notif_0001_inbox_item", INBOX_ITEM_DDL, "notif_inbox_item"),
    ("notif_0002_pref", NOTIF_PREF_DDL, "notif_pref"),
    (
        "notif_0003_quiet_hours",
        QUIET_HOURS_DDL,
        "notif_quiet_hours",
    ),
    ("notif_0004_delivery", DELIVERY_DDL, "notif_delivery"),
    (
        "notif_0005_oncall_schedule",
        ONCALL_SCHEDULE_DDL,
        "notif_oncall_schedule",
    ),
    (
        "notif_0006_escalation_policy",
        ESCALATION_POLICY_DDL,
        "notif_escalation_policy",
    ),
    (
        "notif_0007_escalation_run",
        ESCALATION_RUN_DDL,
        "notif_escalation_run",
    ),
    (
        "notif_0008_humanise_template",
        HUMANISE_TEMPLATE_DDL,
        "notif_humanise_template",
    ),
    ("notif_0009_mute", MUTE_DDL, "notif_mute"),
];

pub fn rls_scope_sql(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

pub fn migrations() -> Migrations {
    let mut migrations = TABLE_DDLS
        .iter()
        .map(|(id, create_ddl, table)| {
            let mut ddl = String::new();
            ddl.push_str(create_ddl);
            ddl.push_str(";\n");
            ddl.push_str(&rls_scope_sql(table));
            ddl.push(';');
            let ddl: &'static str = Box::leak(ddl.into_boxed_str());
            Migration::phased(id, ddl, MigrationPhase::Plain, table)
        })
        .collect::<Vec<_>>();
    migrations.push(Migration::phased(
        INBOX_KEYSET_INDEX_MIGRATION_ID,
        INBOX_KEYSET_INDEX_DDL,
        MigrationPhase::Expand,
        "notif_inbox_item",
    ));
    migrations.push(Migration::phased(
        INBOX_RECENCY_KEYSET_INDEX_MIGRATION_ID,
        INBOX_RECENCY_KEYSET_INDEX_DDL,
        MigrationPhase::Expand,
        "notif_inbox_item",
    ));
    Migrations::of(migrations)
}

pub fn ddl_is_forward_only(ddl: &str) -> bool {
    !myelin_substrate::is_destructive(ddl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{HotTables, MigrationRunner};

    #[test]
    fn notification_migrations_apply_forward_only_in_order() {
        let migrations = migrations();
        assert_eq!(
            migrations.0.len(),
            11,
            "nine tables plus two additive inbox keyset indexes"
        );
        let mut runner = MigrationRunner::new();
        runner
            .run(&migrations, &HotTables::none())
            .expect("the notification migrations apply forward-only");
        assert_eq!(
            runner.applied(),
            &[
                "notif_0001_inbox_item",
                "notif_0002_pref",
                "notif_0003_quiet_hours",
                "notif_0004_delivery",
                "notif_0005_oncall_schedule",
                "notif_0006_escalation_policy",
                "notif_0007_escalation_run",
                "notif_0008_humanise_template",
                "notif_0009_mute",
                "notif_0010_inbox_recipient_keyset",
                "notif_0011_inbox_recipient_recency_keyset",
            ],
            "9 tables + 2 online indexes, in order - 0 backward migration"
        );
    }

    #[test]
    fn every_table_is_tenant_region_first_with_a_tenant_first_pk() {
        for (_id, ddl, table) in TABLE_DDLS {
            let cols = ddl.split('(').nth(1).expect("a column list");
            let first_two: Vec<&str> = cols.split(',').take(2).map(str::trim).collect();
            assert!(
                first_two[0].starts_with("tenant_id text"),
                "first column must be tenant_id ({table}): {ddl}"
            );
            assert!(
                first_two[1].starts_with("region text"),
                "second column must be region ({table}): {ddl}"
            );
            if *table == "notif_humanise_template" {
                assert!(
                    ddl.contains("tenant_scope text GENERATED ALWAYS AS (COALESCE(tenant_id")
                        && ddl.contains("PRIMARY KEY (tenant_scope, region"),
                    "humanise_template has a generated tenant scope PK for the platform-default NULL row"
                );
            } else {
                assert!(
                    ddl.contains("PRIMARY KEY (tenant_id, region"),
                    "the primary key must lead with (tenant_id, region) ({table}): {ddl}"
                );
            }
        }
    }

    #[test]
    fn inbox_item_carries_the_2_1_invariants() {
        let ddl = INBOX_ITEM_DDL;
        for col in [
            "subject text",
            "subject_root text",
            "origin_event text",
            "template_args_json jsonb",
        ] {
            assert!(
                ddl.contains(col),
                "the refs-not-payloads column `{col}` is declared"
            );
        }
        assert_eq!(
            ddl.matches("state text").count(),
            1,
            "inbox_item has EXACTLY ONE read-state column (the C-9 truth, §2.1)"
        );
        assert!(
            ddl.contains("UNIQUE (tenant_id, recipient, dedup_key)"),
            "the UNIQUE(tenant, recipient, dedup_key) write-time-collapse key (§3.2)"
        );
        assert!(
            ddl.contains("coalesce_count integer"),
            "the +N-more counter (NOTIF-P11)"
        );
        assert!(
            ddl.contains("origin_event text"),
            "the NOTIF-2 origin_event provenance"
        );
        assert!(ddl.contains("reason text"), "the NOTIF-2 reason provenance");
    }

    #[test]
    fn delivery_is_at_least_once_idempotent_with_a_redacted_flag() {
        assert!(
            DELIVERY_DDL.contains("UNIQUE (tenant_id, idem_key)"),
            "delivery is idempotent on idem_key (at-least-once + dedup, §2.3)"
        );
        assert!(
            DELIVERY_DDL.contains("redacted boolean"),
            "the off-cell PII-minimisation flag (§3.6)"
        );
    }

    #[test]
    fn every_table_is_encrypted_from_birth() {
        for (_id, ddl, table) in TABLE_DDLS {
            assert!(
                ddl.contains("dek_ref text"),
                "table `{table}` carries the per-row DEK ref: {ddl}"
            );
        }
    }

    #[test]
    fn each_table_gets_the_rls_scope_call() {
        let migrations = migrations();
        for (i, (_id, _ddl, table)) in TABLE_DDLS.iter().enumerate() {
            assert_eq!(
                rls_scope_sql(table),
                format!("SELECT myelin_make_tenant_scoped('{table}')")
            );
            let m = &migrations.0[i];
            assert!(
                m.ddl.contains(&rls_scope_sql(table)),
                "migration `{}` carries the RLS scoping for `{table}`",
                m.id
            );
            assert!(
                m.ddl.contains("CREATE TABLE"),
                "migration `{}` carries the create-table",
                m.id
            );
        }
        assert_eq!(TABLES.len(), 9, "the nine-table data model");
    }

    #[test]
    fn a_destructive_notif_migration_is_refused() {
        let bad = Migrations::of([Migration::plain(
            "notif_9999_drop",
            "DROP TABLE notif_inbox_item",
        )]);
        let mut runner = MigrationRunner::new();
        let e = runner
            .run(&bad, &HotTables::none())
            .expect_err("a DROP must be refused");
        assert!(
            e.0.contains("forward-only"),
            "the refusal names forward-only: {}",
            e.0
        );
        for (_id, ddl, _table) in TABLE_DDLS {
            assert!(ddl_is_forward_only(ddl), "the real DDL is forward-only");
            assert!(
                !ddl.to_ascii_uppercase().contains("DROP"),
                "no DROP in the data-model DDL"
            );
        }
    }
}
