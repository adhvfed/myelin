import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { CiRouteError, getCiLog, getCiRun } from "~/lib/api";
import { isCiUuid } from "~/lib/ci-read-input";
import { ciRepoLabel, type CiJobVM, type CiStepVM } from "~/lib/ci-read-response";
import { CiErrorState, ciErrKind } from "~/components/CiErrorState";
import { CiLiveLog } from "~/components/CiLiveLog";
import { CiStatus, formatCiDate } from "~/components/CiStatus";

const ARCHIVE_CHUNK = 64 * 1024;

export default function CiRunDetail() {
  const params = useParams();
  const [search] = useSearchParams();
  const runId = () => params.run ?? "";
  const detail = createAsync(() => getCiRun(runId()), { deferStream: true });
  const selectedJob = () => typeof search.job === "string" ? search.job : undefined;
  const selectedOffset = () => {
    if (search.offset === undefined) return 0;
    if (typeof search.offset !== "string" || !/^(?:0|[1-9][0-9]*)$/.test(search.offset)) {
      return null;
    }
    const value = Number(search.offset);
    return Number.isSafeInteger(value) ? value : null;
  };
  const archivedLog = createAsync(async () => {
    const job = selectedJob();
    if (job === undefined) return undefined;
    const offset = selectedOffset();
    if (!isCiUuid(job) || offset === null) throw new CiRouteError("bad-input");
    return getCiLog({ run: runId(), job, start: offset, limit: ARCHIVE_CHUNK });
  }, { deferStream: true });

  return (
    <section aria-labelledby="ci-run-heading" class="ci-screen">
      <Title>CI run · Myelin</Title>
      <nav aria-label="Breadcrumb" class="ci-breadcrumb">
        <A href="/ci">CI runs</A>
        <span aria-hidden="true">/</span>
        <span aria-current="page">{runId().slice(0, 8)}</span>
      </nav>

      <ErrorBoundary fallback={(error, reset) => <CiErrorState kind={ciErrKind(error)} onRetry={reset} />}>
        <Suspense
          fallback={
            <Skeleton
              label="Loading CI run…"
              rows={5}
              rowHeight="4rem"
              data-testid="ci-run-loading"
            />
          }
        >
          <Show when={detail()} keyed>
            {(view) => (
              <>
                <header class="ci-run-detail-header">
                  <div>
                    <p class="ci-eyebrow">{ciRepoLabel(view.run.repo_ref)}</p>
                    <h1 id="ci-run-heading">Run {view.run.run_id.slice(0, 8)}</h1>
                    <p>
                      <code>{view.run.commit_oid ?? "No commit object"}</code>
                      <span aria-hidden="true"> · </span>
                      <time datetime={view.run.created_at}>{formatCiDate(view.run.created_at)}</time>
                    </p>
                  </div>
                  <CiStatus state={view.run.state} />
                </header>

                <dl class="ci-run-facts">
                  <div><dt>Trigger</dt><dd>{view.run.trigger_kind.replaceAll("_", " ")}</dd></div>
                  <div><dt>Trust</dt><dd>{view.run.trust_tier.replaceAll("_", " ")}</dd></div>
                  <div><dt>Accounting</dt><dd>{view.run.cost_settled ? "Settled" : "Pending"}</dd></div>
                  <div><dt>Pipeline</dt><dd><code>{view.run.pipeline_id.slice(0, 8)}</code></dd></div>
                </dl>

                <section aria-labelledby="ci-jobs-heading" class="ci-jobs">
                  <h2 id="ci-jobs-heading">Jobs</h2>
                  <Show
                    when={view.jobs.length > 0}
                    fallback={<p data-testid="ci-jobs-empty">This run has no materialized jobs.</p>}
                  >
                    <ul>
                      <For each={view.jobs}>
                        {(job) => (
                          <JobRow
                            run={view.run.run_id}
                            job={job}
                            steps={view.steps.filter((step) => step.job_id === job.job_id)}
                            selected={selectedJob() === job.job_id}
                          />
                        )}
                      </For>
                    </ul>
                  </Show>
                </section>

                <section aria-labelledby="archived-log-heading" id="archived-log" class="ci-log-section">
                  <div class="ci-log-heading">
                    <div>
                      <p class="ci-eyebrow">Cold-path read</p>
                      <h2 id="archived-log-heading">Archived output</h2>
                    </div>
                    <span class="ci-archive-label"><Icon name="database" /> Archived</span>
                  </div>
                  <Show
                    when={selectedJob()}
                    fallback={<p data-testid="ci-log-unselected">Choose a job to read its archived output.</p>}
                    keyed
                  >
                    {(job) => (
                      <>
                        <CiLiveLog run={view.run.run_id} job={job} />
                        <ErrorBoundary
                          fallback={(error, reset) => (
                            <CiErrorState kind={ciErrKind(error)} onRetry={reset} />
                          )}
                        >
                          <Suspense
                            fallback={
                              <Skeleton
                                label="Loading archived output…"
                                rows={4}
                                rowHeight="1rem"
                                data-testid="ci-log-loading"
                              />
                            }
                          >
                            <Show when={archivedLog()} keyed>
                              {(range) => (
                                <>
                                  <p class="ci-log-range">
                                    Archived bytes {range.byte_start}–{range.byte_end} of {range.total_end}
                                  </p>
                                  <textarea
                                    readOnly
                                    aria-label="Archived job output"
                                    data-testid="ci-archived-log"
                                    value={range.text.length > 0 ? range.text : "No archived bytes in this range."}
                                  />
                                  <nav aria-label="Archived log ranges" class="ci-pagination">
                                    <Show when={range.byte_start > 0}>
                                      <A href={logHref(view.run.run_id, range.job_id, 0)}>
                                        Start of log
                                      </A>
                                    </Show>
                                    <Show when={range.next_offset !== null}>
                                      <A href={logHref(view.run.run_id, range.job_id, range.next_offset!)}>
                                        Next archived chunk <Icon name="chevron" />
                                      </A>
                                    </Show>
                                  </nav>
                                </>
                              )}
                            </Show>
                          </Suspense>
                        </ErrorBoundary>
                      </>
                    )}
                  </Show>
                </section>
              </>
            )}
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function JobRow(props: {
  run: string;
  job: CiJobVM;
  steps: CiStepVM[];
  selected: boolean;
}) {
  return (
    <li id={`job-${props.job.job_id}`} class="ci-job" data-selected={props.selected ? "true" : "false"}>
      <div class="ci-job-heading">
        <div>
          <span class="ci-eyebrow">{props.job.stage}</span>
          <h3>{props.job.name}</h3>
        </div>
        <CiStatus state={props.job.state} />
      </div>
      <p>Attempt {props.job.attempt}</p>
      <Show when={props.steps.length > 0}>
        <ol class="ci-step-list">
          <For each={props.steps}>
            {(step) => (
              <li id={`step-${step.step_id}`}>
                <CiStatus state={step.status} />
                <span>{step.step_id}</span>
                <span>byte {step.byte_start}</span>
              </li>
            )}
          </For>
        </ol>
      </Show>
      <A href={logHref(props.run, props.job.job_id, 0)} class="ci-secondary-action">
        <Icon name="database" /> Read archived output
      </A>
    </li>
  );
}

function logHref(run: string, job: string, offset: number): string {
  const query = new URLSearchParams({ job });
  if (offset > 0) query.set("offset", String(offset));
  return `/ci/runs/${run}?${query.toString()}#archived-log`;
}
