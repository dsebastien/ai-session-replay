import {
  MAX_FRAME_COUNT,
  MAX_OUTPUT_DURATION_MS,
  MAX_SOURCE_DURATION_MS,
  MAX_SPEED,
  MIN_SPEED,
  type SpeedSegment,
} from "@ai-session-replay/replay-contract";

export function outputDurationMs(
  segments: readonly SpeedSegment[],
): number {
  assertSegments(segments);

  const duration = segments.reduce(
    (total, segment) =>
      total +
      (segment.sourceEndMs - segment.sourceStartMs) / segment.speed,
    0,
  );

  if (!Number.isFinite(duration) || duration > MAX_OUTPUT_DURATION_MS) {
    throw new RangeError("Output duration exceeds the supported range");
  }

  return duration;
}

export function sourceTimeAtOutputMs(
  segments: readonly SpeedSegment[],
  outputMs: number,
): number {
  assertSegments(segments);
  const target = clamp(outputMs, 0, outputDurationMs(segments));
  let outputCursor = 0;

  for (const segment of segments) {
    const segmentOutputDuration =
      (segment.sourceEndMs - segment.sourceStartMs) / segment.speed;
    const outputEnd = outputCursor + segmentOutputDuration;

    if (target <= outputEnd) {
      return Math.min(
        segment.sourceEndMs,
        segment.sourceStartMs + (target - outputCursor) * segment.speed,
      );
    }
    outputCursor = outputEnd;
  }

  return requiredLast(segments).sourceEndMs;
}

export function outputTimeAtSourceMs(
  segments: readonly SpeedSegment[],
  sourceMs: number,
): number {
  assertSegments(segments);
  const first = requiredFirst(segments);
  const target = clamp(sourceMs, first.sourceStartMs, requiredLast(segments).sourceEndMs);
  let outputCursor = 0;

  for (const segment of segments) {
    if (target <= segment.sourceEndMs) {
      return (
        outputCursor + (target - segment.sourceStartMs) / segment.speed
      );
    }
    outputCursor +=
      (segment.sourceEndMs - segment.sourceStartMs) / segment.speed;
  }

  return outputCursor;
}

export function frameCount(durationMs: number, fps: number): number {
  if (
    !Number.isFinite(durationMs) ||
    durationMs <= 0 ||
    !Number.isSafeInteger(fps) ||
    fps <= 0
  ) {
    throw new RangeError("Duration and frame rate must be positive");
  }

  const frames = Math.ceil((durationMs / 1_000) * fps);
  if (!Number.isSafeInteger(frames) || frames > MAX_FRAME_COUNT) {
    throw new RangeError("Frame count exceeds the supported range");
  }

  return Math.max(1, frames);
}

function assertSegments(segments: readonly SpeedSegment[]): void {
  if (segments.length === 0) {
    throw new RangeError("At least one speed segment is required");
  }

  const ids = new Set<string>();
  let expectedStart = segments[0]?.sourceStartMs;
  for (const segment of segments) {
    if (
      !Number.isSafeInteger(segment.sourceStartMs) ||
      !Number.isSafeInteger(segment.sourceEndMs) ||
      segment.sourceStartMs < 0 ||
      segment.sourceEndMs > MAX_SOURCE_DURATION_MS ||
      segment.sourceStartMs !== expectedStart ||
      segment.sourceStartMs >= segment.sourceEndMs ||
      !Number.isFinite(segment.speed) ||
      segment.speed < MIN_SPEED ||
      segment.speed > MAX_SPEED ||
      segment.id.length === 0 ||
      ids.has(segment.id)
    ) {
      throw new RangeError("Speed segments must form a contiguous timeline");
    }
    ids.add(segment.id);
    expectedStart = segment.sourceEndMs;
  }
}

function requiredFirst(
  segments: readonly SpeedSegment[],
): SpeedSegment {
  const first = segments[0];
  if (first === undefined) {
    throw new RangeError("At least one speed segment is required");
  }
  return first;
}

function requiredLast(
  segments: readonly SpeedSegment[],
): SpeedSegment {
  const last = segments.at(-1);
  if (last === undefined) {
    throw new RangeError("At least one speed segment is required");
  }
  return last;
}

function clamp(value: number, minimum: number, maximum: number): number {
  if (!Number.isFinite(value)) {
    throw new RangeError("Timeline position must be finite");
  }
  return Math.min(maximum, Math.max(minimum, value));
}
