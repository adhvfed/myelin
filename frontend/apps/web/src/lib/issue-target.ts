export interface IssueTargetEnv {
  MYELIN_ISSUES_PROJECT?: string;
  MYELIN_ISSUES_TYPE?: string;
  MYELIN_ISSUES_PREFIX?: string;
}

export interface IssueTarget {
  project_id: string;
  type_id: string;
  prefix: string;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const PREFIX = /^[A-Z0-9]{2,10}$/;

/** Parse the deployment's default issue destination without exposing partial configuration. */
export function issueTargetFromEnv(env: IssueTargetEnv): IssueTarget | null {
  const project = env.MYELIN_ISSUES_PROJECT?.trim();
  const type = env.MYELIN_ISSUES_TYPE?.trim();
  const prefix = env.MYELIN_ISSUES_PREFIX?.trim();
  if (!project || !type || !prefix || !UUID.test(project) || !UUID.test(type) || !PREFIX.test(prefix)) {
    return null;
  }
  return { project_id: project, type_id: type, prefix };
}
