// @vitest-environment node

import {describe, expect, it} from "vitest";
import {
  frameCount,
  outputDurationMs,
  outputTimeAtSourceMs,
  sourceTimeAtOutputMs,
} from "./index";
import type {SpeedSegment} from "@ai-session-replay/replay-contract";

const segments: readonly SpeedSegment[] = [
  {
    id: "normal",
    sourceStartMs: 0,
    sourceEndMs: 1_000,
    speed: 1,
  },
  {
    id: "fast",
    sourceStartMs: 1_000,
    sourceEndMs: 3_000,
    speed: 2,
  },
];

describe("replay timeline", () => {
  it("computes duration across segments with different speeds", () => {
    expect(outputDurationMs(segments)).toBe(2_000);
  });

  it("maps output time to source time at segment boundaries", () => {
    expect(sourceTimeAtOutputMs(segments, 1_000)).toBe(1_000);
    expect(sourceTimeAtOutputMs(segments, 1_500)).toBe(2_000);
  });

  it("maps source time back to output time", () => {
    expect(outputTimeAtSourceMs(segments, 2_000)).toBe(1_500);
    expect(outputTimeAtSourceMs(segments, 3_000)).toBe(2_000);
  });

  it("rounds partial frames up so the final hold is renderable", () => {
    expect(frameCount(1_501, 30)).toBe(46);
  });

  it("clamps positions before the start and after the end", () => {
    expect(sourceTimeAtOutputMs(segments, -100)).toBe(0);
    expect(sourceTimeAtOutputMs(segments, 9_000)).toBe(3_000);
    expect(outputTimeAtSourceMs(segments, -100)).toBe(0);
    expect(outputTimeAtSourceMs(segments, 9_000)).toBe(2_000);
  });

  it("rejects empty timelines and non-finite positions", () => {
    expect(() => outputDurationMs([])).toThrow(
      "At least one speed segment is required",
    );
    expect(() => sourceTimeAtOutputMs(segments, Number.NaN)).toThrow(
      "Timeline position must be finite",
    );
  });

  it("rejects invalid frame metadata", () => {
    expect(() => frameCount(0, 30)).toThrow(
      "Duration and frame rate must be positive",
    );
    expect(() => frameCount(7_200_001, 30)).toThrow(
      "Frame count exceeds the supported range",
    );
  });

  it("rejects reversed, gapped, and overlapping segment layouts", () => {
    expect(() =>
      outputDurationMs([
        {id: "reversed", sourceStartMs: 1_000, sourceEndMs: 0, speed: 1},
      ]),
    ).toThrow("Speed segments must form a contiguous timeline");
    expect(() =>
      outputDurationMs([
        {id: "one", sourceStartMs: 0, sourceEndMs: 1_000, speed: 1},
        {id: "two", sourceStartMs: 1_001, sourceEndMs: 2_000, speed: 1},
      ]),
    ).toThrow("Speed segments must form a contiguous timeline");
    expect(() =>
      outputDurationMs([
        {id: "one", sourceStartMs: 0, sourceEndMs: 1_000, speed: 1},
        {id: "two", sourceStartMs: 999, sourceEndMs: 2_000, speed: 1},
      ]),
    ).toThrow("Speed segments must form a contiguous timeline");
  });
});
