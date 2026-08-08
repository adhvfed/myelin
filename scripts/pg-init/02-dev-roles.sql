-- Reconcile local-development roles when a Fed data volume outlives changes to
-- the initial role definitions. Production credentials are managed outside
-- this development-only bootstrap.
\set ON_ERROR_STOP on

ALTER ROLE myelin_app PASSWORD 'myelin_app_pw';
ALTER ROLE myelin_ci_scheduler_fr_par PASSWORD 'myelin_ci_scheduler_dev_pw';
ALTER ROLE myelin_outbox_publisher_fr_par PASSWORD 'myelin_outbox_publisher_dev_pw';

DO $$
BEGIN
  IF to_regclass('public.outbox') IS NOT NULL THEN
    REVOKE ALL PRIVILEGES ON TABLE public.outbox FROM myelin_outbox_publisher;
    GRANT SELECT ON TABLE public.outbox TO myelin_outbox_publisher;
    GRANT UPDATE (published_at) ON TABLE public.outbox TO myelin_outbox_publisher;
  END IF;

  IF to_regclass('public.outbox_quarantine') IS NOT NULL THEN
    REVOKE ALL PRIVILEGES ON TABLE public.outbox_quarantine FROM myelin_outbox_publisher;
    GRANT SELECT, INSERT ON TABLE public.outbox_quarantine TO myelin_outbox_publisher;
  END IF;
END
$$;
