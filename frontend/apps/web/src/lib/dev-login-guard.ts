// Dependency-free development-login guard shared by server code and Node tests.

/** The environment facts the dev-login decision reads. */
export interface DevLoginEnv {
  NODE_ENV?: string;
  MYELIN_DEV_LOGIN?: string;
}

/**
 * Development login requires a non-production environment and `MYELIN_DEV_LOGIN=1`.
 * Missing or unexpected values return false.
 */
export function devLoginAllowed(env: DevLoginEnv): boolean {
  const isProduction = env.NODE_ENV === "production";
  const explicitlyOptedIn = env.MYELIN_DEV_LOGIN === "1";
  return !isProduction && explicitlyOptedIn;
}

/**
 * Render development login only when the build, frontend environment, and edge config all allow it.
 */
export function devSeamAllowed(
  edgeDevLoginEnabled: boolean,
  env: DevLoginEnv,
  isProdBuild: boolean,
): boolean {
  return !isProdBuild && devLoginAllowed(env) && edgeDevLoginEnabled;
}
