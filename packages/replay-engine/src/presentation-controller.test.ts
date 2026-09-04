import {describe, expect, it} from "vitest";
import {
  initialPresentationState,
  playbackDelayUntil,
  presentationIndexAtElapsed,
  reducePresentation,
} from "./presentation-controller";

describe("presentation controller", () => {
  it("supports play, pause, stepping, and boundary jumps", () => {
    let state = initialPresentationState(4);
    state = reducePresentation(state, {type: "play"}, 4);
    expect(state).toEqual({status: "playing", index: 0});
    state = reducePresentation(state, {type: "tick"}, 4);
    expect(state).toEqual({status: "playing", index: 1});
    state = reducePresentation(state, {type: "pause"}, 4);
    state = reducePresentation(state, {type: "next"}, 4);
    expect(state).toEqual({status: "paused", index: 2});
    state = reducePresentation(state, {type: "last"}, 4);
    expect(state).toEqual({status: "complete", index: 3});
    state = reducePresentation(state, {type: "previous"}, 4);
    expect(state).toEqual({status: "paused", index: 2});
    expect(reducePresentation(state, {type: "first"}, 4)).toEqual({status: "paused", index: 0});
  });

  it("finishes at the last entry and rejects empty plans", () => {
    expect(() => initialPresentationState(0)).toThrow("at least one entry");
    const last = {status: "playing" as const, index: 1};
    expect(reducePresentation(last, {type: "tick"}, 2)).toEqual({status: "complete", index: 1});
    expect(reducePresentation(last, {type: "next"}, 2)).toEqual({status: "complete", index: 1});
    expect(reducePresentation({status: "complete", index: 1}, {type: "play"}, 2)).toEqual({status: "playing", index: 0});
    expect(reducePresentation({status: "playing", index: 0}, {type: "toggle"}, 2)).toEqual({status: "paused", index: 0});
    expect(reducePresentation({status: "complete", index: 1}, {type: "toggle"}, 2)).toEqual({status: "playing", index: 0});
    expect(reducePresentation({status: "paused", index: 0}, {type: "toggle"}, 2)).toEqual({status: "playing", index: 0});
    expect(() => reducePresentation(last, {type: "tick"}, 1.5)).toThrow("at least one entry");
  });

  it("schedules against an absolute deadline so delayed callbacks do not accumulate drift", () => {
    expect(playbackDelayUntil(1_000, 2_000, 2_250)).toBe(750);
    expect(playbackDelayUntil(1_000, 2_000, 3_100)).toBe(1);
    expect(() => playbackDelayUntil(Number.NaN, 2_000, 1_000)).toThrow("finite");
    expect(presentationIndexAtElapsed([0, 1_000, 2_000, 3_000], 2_750)).toBe(2);
    expect(reducePresentation({status: "playing", index: 1}, {type: "sync", index: -2, complete: false}, 4)).toEqual({status: "playing", index: 0});
    expect(reducePresentation({status: "playing", index: 1}, {type: "sync", index: 99, complete: true}, 4)).toEqual({status: "complete", index: 3});
    expect(() => reducePresentation({status: "playing", index: 1}, {type: "sync", index: 1.5, complete: false}, 4)).toThrow("invalid");
    expect(() => presentationIndexAtElapsed([], 0)).toThrow("invalid");
  });
});
