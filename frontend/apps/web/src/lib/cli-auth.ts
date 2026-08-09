import { action, redirect } from "@solidjs/router";

import {
  edgeApproveCliLogin,
  GatewayError,
  isUnauthorized,
} from "../server/gateway";
import { getSessionRecord } from "../server/session";
import { authenticationDestination } from "./auth-return";
import {
  canonicalCliUserCode,
  cliApprovalPath,
  type CliApprovalResult,
} from "./cli-auth-core";

function failedApproval(error: unknown): CliApprovalResult {
  if (error instanceof GatewayError) {
    if (error.status === 404) return "expired";
    if (error.status === 403) return "forbidden";
  }
  return "unavailable";
}

/**
 * Approve a pending CLI login with the browser's server-side session capability. The action sends
 * only the human code to the browser and only the session token to Edge; neither crosses the other
 * boundary.
 */
export const approveCliLogin = action(async (formData: FormData) => {
  "use server";
  const userCode = canonicalCliUserCode(formData.get("user_code"));
  if (!userCode) throw redirect("/cli/auth?result=expired");

  const approvalPath = cliApprovalPath(userCode);
  if (!(await getSessionRecord())) throw redirect(authenticationDestination(approvalPath));

  try {
    await edgeApproveCliLogin(userCode);
  } catch (error) {
    if (isUnauthorized(error)) throw redirect(authenticationDestination(approvalPath));
    throw redirect(cliApprovalPath(userCode, failedApproval(error)));
  }
  throw redirect(cliApprovalPath(userCode, "approved"));
}, "approve-cli-login");
