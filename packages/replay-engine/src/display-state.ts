import type {ReplayEvent} from "@ai-session-replay/replay-contract";

export interface ReplayDisplayState {
  readonly visibleEvents: readonly ReplayEvent[];
  readonly activeEventId: string | null;
}

export function displayStateAtSourceMs(
  events: readonly ReplayEvent[],
  sourceMs: number,
): ReplayDisplayState {
  if (!Number.isFinite(sourceMs) || sourceMs < 0) {
    throw new RangeError("Source time must be finite and non-negative");
  }

  let visibleCount = 0;
  while (visibleCount < events.length) {
    const event = events[visibleCount];
    if (event === undefined || event.atMs > sourceMs) {
      break;
    }
    visibleCount += 1;
  }

  const visibleEvents = events.slice(0, visibleCount);
  return {
    visibleEvents,
    activeEventId: visibleEvents.at(-1)?.id ?? null,
  };
}
