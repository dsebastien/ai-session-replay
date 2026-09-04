export type PresentationPlaybackState = Readonly<{
  status: "paused" | "playing" | "complete";
  index: number;
}>;

export type PresentationPlaybackAction =
  | Readonly<{type: "play" | "pause" | "toggle" | "next" | "previous" | "first" | "last" | "tick"}>
  | Readonly<{type: "sync"; index: number; complete: boolean}>;

export function initialPresentationState(entryCount: number): PresentationPlaybackState {
  assertEntryCount(entryCount);
  return {status: "paused", index: 0};
}

export function reducePresentation(
  state: PresentationPlaybackState,
  action: PresentationPlaybackAction,
  entryCount: number,
): PresentationPlaybackState {
  assertEntryCount(entryCount);
  const last = entryCount - 1;
  switch (action.type) {
    case "play":
      return state.index >= last ? {status: "playing", index: 0} : {...state, status: "playing"};
    case "pause":
      return {...state, status: "paused"};
    case "toggle":
      return state.status === "playing"
        ? {...state, status: "paused"}
        : state.index >= last
          ? {status: "playing", index: 0}
          : {...state, status: "playing"};
    case "next":
    case "tick":
      return state.index >= last
        ? {status: "complete", index: last}
        : {status: action.type === "tick" ? state.status : "paused", index: state.index + 1};
    case "previous":
      return {status: "paused", index: Math.max(0, state.index - 1)};
    case "first":
      return {status: "paused", index: 0};
    case "last":
      return {status: "complete", index: last};
    case "sync": {
      if (!Number.isSafeInteger(action.index)) throw new RangeError("Presentation index is invalid");
      return {
        status: action.complete ? "complete" : "playing",
        index: Math.max(0, Math.min(last, action.index)),
      };
    }
  }
}

export function playbackDelayUntil(
  playbackOriginMs: number,
  nextRevealAtMs: number,
  nowMs: number,
): number {
  if (![playbackOriginMs, nextRevealAtMs, nowMs].every(Number.isFinite)) {
    throw new RangeError("Playback clock values must be finite");
  }
  return Math.max(1, Math.round(playbackOriginMs + nextRevealAtMs - nowMs));
}

export function presentationIndexAtElapsed(
  revealTimes: readonly number[],
  elapsedMs: number,
): number {
  if (!Number.isFinite(elapsedMs) || elapsedMs < 0 || revealTimes.length === 0) {
    throw new RangeError("Presentation time is invalid");
  }
  let lower = 0;
  let upper = revealTimes.length;
  while (lower < upper) {
    const middle = Math.floor((lower + upper) / 2);
    if ((revealTimes[middle] ?? Number.POSITIVE_INFINITY) <= elapsedMs) lower = middle + 1;
    else upper = middle;
  }
  return Math.max(0, lower - 1);
}

function assertEntryCount(entryCount: number): void {
  if (!Number.isSafeInteger(entryCount) || entryCount < 1) {
    throw new RangeError("Presentation requires at least one entry");
  }
}
