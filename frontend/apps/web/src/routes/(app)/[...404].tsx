import { Title } from "@solidjs/meta";
import { NotAvailable } from "~/components/NotAvailable";

export default function CatchAll() {
  return (
    <>
      <Title>Not found · Myelin</Title>
      <NotAvailable subsystem="This page" status="missing" />
    </>
  );
}
