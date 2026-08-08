import { Title } from "@solidjs/meta";
import { NotAvailable } from "~/components/NotAvailable";

export default function ChatIndex() {
  return (
    <>
      <Title>Chat · Myelin</Title>
      <NotAvailable subsystem="Chat" />
    </>
  );
}
