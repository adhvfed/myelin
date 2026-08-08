import { Navigate } from "@solidjs/router";

// The app root redirects to the one real, edge-backed screen (the repos list).
export default function Index() {
  return <Navigate href="/git/repos" />;
}
