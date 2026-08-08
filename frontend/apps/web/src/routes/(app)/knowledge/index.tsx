import { Title } from "@solidjs/meta";
import { NotAvailable } from "~/components/NotAvailable";

export default function KnowledgeIndex() {
  return (
    <>
      <Title>Knowledge · Myelin</Title>
      <NotAvailable subsystem="Knowledge" />
    </>
  );
}
