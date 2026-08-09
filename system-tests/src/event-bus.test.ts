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
  });

  test("refuses ambiguous or non-canonical routing components", () => {
    expect(() => eventBusSubject({
      event_id: "event-1",
      type_: "signal.opened.extra",
      tenant: "tenant-one",
      aggregate: "signal:dedup-one",
    })).toThrow("exactly two components");
    expect(() => eventBusSubject({
      event_id: "event-1",
      type_: "signal.opened",
      tenant: "tenant.one",
      aggregate: "signal:dedup-one",
    })).toThrow("canonical event-bus token");
  });
});
