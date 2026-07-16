// Unbuilt-subsystem index (R3.4 / firstrun #1) — renders the teaching NotAvailable INSIDE the shell.
// This surface lands with the CI subsystem track; until then it is an honest "not here yet" page,
// keyboard-reachable from the rail (never a dead link, never a raw framework 404).
import { Title } from "@solidjs/meta";
import { NotAvailable } from "~/components/NotAvailable";

export default function CIIndex() {
  return (
    <>
      <Title>CI · Myelin</Title>
      <NotAvailable subsystem="CI" />
    </>
  );
}
