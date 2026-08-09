import { jetstream, type JetStreamClient, type PubAck } from "@nats-io/jetstream";
import { connect, type NatsConnection } from "@nats-io/transport-node";

export interface ExternalEventEnvelope {
  event_id: string;
  type_: string;
  tenant: string;
  aggregate: string;
  [field: string]: unknown;
}

function token(value: string, field: string): string {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) {
    throw new Error(`${field} cannot be represented as a canonical event-bus token`);
  }
  return value;
}

export function eventBusSubject(envelope: ExternalEventEnvelope): string {
  const [subsystem, eventName, ...extraType] = envelope.type_.split(".");
  const [aggregateType, aggregateId, ...extraAggregate] = envelope.aggregate.split(":");
  if (
    !subsystem || !eventName || extraType.length > 0 ||
    !aggregateType || !aggregateId || extraAggregate.length > 0
  ) {
    throw new Error("event type and aggregate must each contain exactly two components");
  }
  return [
    "myelin",
    "events",
    "evt",
    token(envelope.tenant, "tenant"),
    token(subsystem, "subsystem"),
    token(aggregateType, "aggregate type"),
    token(aggregateId, "aggregate id"),
    token(eventName, "event name"),
  ].join(".");
}

export class ExternalEventBus {
  private constructor(
    private readonly connection: NatsConnection,
    private readonly stream: JetStreamClient,
  ) {}

  static async connect(url: string): Promise<ExternalEventBus> {
    const connection = await connect({ servers: url, name: "myelin-system-tests" });
    return new ExternalEventBus(connection, jetstream(connection));
  }

  async publish(envelope: ExternalEventEnvelope): Promise<PubAck> {
    return this.stream.publish(
      eventBusSubject(envelope),
      new TextEncoder().encode(JSON.stringify(envelope)),
      { msgID: envelope.event_id },
    );
  }

  async close(): Promise<void> {
    await this.connection.close();
  }
}
