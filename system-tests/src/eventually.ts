export interface EventuallyOptions {
  timeoutMs?: number;
  intervalMs?: number;
  description: string;
}

export async function eventually<T>(
  probe: () => Promise<T | undefined>,
  options: EventuallyOptions,
): Promise<T> {
  const timeoutMs = options.timeoutMs ?? 15_000;
  const intervalMs = options.intervalMs ?? 100;
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;

  while (Date.now() < deadline) {
    try {
      const result = await probe();
      if (result !== undefined) return result;
      lastError = undefined;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }

  const detail = lastError instanceof Error ? ` Last error: ${lastError.message}` : "";
  throw new Error(`Timed out after ${timeoutMs}ms waiting for ${options.description}.${detail}`);
}
