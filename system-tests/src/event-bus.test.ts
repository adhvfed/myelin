import { describe, expect, test } from "vitest";

import { eventBusSubject } from "./event-bus.js";

describe("external event-bus routing", () => {
  test("matches the backend's partitioned subject grammar", () => {
    expect(eventBusSubject({
      event_id: "event-1",
      type_: "signal.opened",
      tenant: "tenant-one",
      aggregate: "signal:dedup-one",
    })).toBe("myelin.events.evt.tenant-one.signal.signal.dedup-one.opened");

    expect(eventBusSubject({
      event_id: "event-2",
      type_: "ci.run.failed",
      tenant: "tenant-one",
      aggregate: "run:run-one",
    })).toBe("myelin.events.evt.tenant-one.ci.run.run-one.failed");
  });

  test("refuses ambiguous or non-canonical routing components", () => {
    expect(() => eventBusSubject({
      event_id: "event-1",
      type_: "opened",
      tenant: "tenant-one",
      aggregate: "signal:dedup-one",
    })).toThrow("at least two components");
    expect(() => eventBusSubject({
      event_id: "event-1",
      type_: "signal.opened",
      tenant: "tenant.one",
      aggregate: "signal:dedup-one",
    })).toThrow("canonical event-bus token");
  });
});
