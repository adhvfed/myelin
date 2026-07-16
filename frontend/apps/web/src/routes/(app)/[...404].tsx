// The catch-all route (R3.4 / firstrun #1) — any unmatched in-shell path renders the teaching
// NotAvailable INSIDE the app shell (the rail + chrome stay), NOT a bare framework 404. So a bad or
// not-yet-built URL lands somewhere dignified and navigable, never a dead end. Semantic tokens only.
import { Title } from "@solidjs/meta";
import { NotAvailable } from "~/components/NotAvailable";

export default function CatchAll() {
  return (
    <>
      <Title>Not available · Myelin</Title>
      <NotAvailable subsystem="This page" />
    </>
  );
}
