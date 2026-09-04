// @vitest-environment node

import {describe, expect, it} from "vitest";
import {
  DEFAULT_TERMINAL_HOLD_MS,
  MAX_EVENT_COUNT,
  createReplayProject,
  validateNormalizedSession,
  validateReplayProject,
  type NormalizedSessionV1,
} from "./index";
import contractFixture from "../../../tests/fixtures/contracts-v1.json";

function session(
  events: NormalizedSessionV1["events"],
): NormalizedSessionV1 {
  return {
    schemaVersion: 1,
    id: "session-1",
    source: "codex",
    sourceVersion: "1.0.0",
    title: "Build replay editor",
    createdAt: "2026-08-24T08:00:00.000Z",
    cwd: "C:\\code\\replay",
    relationships: [],
    events,
    durationMs:
      (events.at(-1)?.atMs ?? 0) + DEFAULT_TERMINAL_HOLD_MS,
    diagnostics: [],
    unknownRecordCount: 0,
  };
}

describe("createReplayProject", () => {
  it("keeps a single event visible for the terminal hold", () => {
    const project = createReplayProject(
      session([{id: "event-1", atMs: 0, kind: "user", text: "Ship it"}]),
    );

    expect(project.trim).toEqual({
      startMs: 0,
      endMs: DEFAULT_TERMINAL_HOLD_MS,
    });
    expect(project.segments).toEqual([
      {
        id: "segment-1",
        sourceStartMs: 0,
        sourceEndMs: DEFAULT_TERMINAL_HOLD_MS,
        speed: 1,
      },
    ]);
  });

  it("rejects sessions without replayable events", () => {
    expect(() => createReplayProject(session([]))).toThrow(
      "Session has no replayable events",
    );
  });
});

describe("validateNormalizedSession", () => {
  it("accepts a complete session and rejects malformed event payloads", () => {
    const valid = session([
      {id: "event-1", atMs: 0, kind: "user", text: "Ship it"},
    ]);

    expect(validateNormalizedSession(valid)).toEqual({ok: true, value: valid});
    expect(
      validateNormalizedSession({
        ...valid,
        events: [{id: "event-1", atMs: 0, kind: "user", text: 42}],
      }).ok,
    ).toBe(false);
  });
});

describe("validateReplayProject", () => {
  it("accepts the cross-language contract fixture", () => {
    expect(validateReplayProject(contractFixture)).toEqual({
      ok: true,
      value: contractFixture,
    });
  });

  it("rejects speed segments that do not exactly partition the trim", () => {
    const project = createReplayProject(
      session([
        {id: "event-1", atMs: 0, kind: "user", text: "Start"},
        {id: "event-2", atMs: 1_000, kind: "assistant", markdown: "Done"},
      ]),
    );

    const result = validateReplayProject({
      ...project,
      segments: [
        {
          id: "segment-1",
          sourceStartMs: 100,
          sourceEndMs: project.trim.endMs,
          speed: 1,
        },
      ],
    });

    expect(result).toEqual({
      ok: false,
      error: {
        code: "INVALID_SEGMENT_PARTITION",
        message: "Speed segments must exactly partition the trim range",
      },
    });
  });

  it("rejects projects over the event limit before rendering", () => {
    const baseEvent = {
      id: "event",
      atMs: 0,
      kind: "unknown" as const,
      sourceType: "fixture",
    };
    const project = createReplayProject(session([baseEvent]));

    const result = validateReplayProject({
      ...project,
      session: {
        ...project.session,
        events: Array.from({length: MAX_EVENT_COUNT + 1}, (_, index) => ({
          ...baseEvent,
          id: `event-${index}`,
        })),
      },
    });

    expect(result).toMatchObject({
      ok: false,
      error: {code: "EVENT_LIMIT_EXCEEDED"},
    });
  });

  it("rejects event objects that omit their discriminated payload", () => {
    const project = createReplayProject(
      session([{id: "event-1", atMs: 0, kind: "user", text: "Start"}]),
    );

    const result = validateReplayProject({
      ...project,
      session: {
        ...project.session,
        events: [{id: "event-1", atMs: 0, kind: "tool"}],
      },
    });

    expect(result).toMatchObject({
      ok: false,
      error: {code: "INVALID_EVENT"},
    });
  });

  it("defaults long sessions to a valid final two-hour trim", () => {
    const longSession = session([
      {id: "event-1", atMs: 0, kind: "user", text: "Start"},
      {
        id: "event-2",
        atMs: 10_800_000,
        kind: "assistant",
        markdown: "Finish",
      },
    ]);

    const project = createReplayProject(longSession);

    expect(project.trim).toEqual({
      startMs: 3_601_500,
      endMs: 10_801_500,
    });
    expect(validateReplayProject(project).ok).toBe(true);
  });

  it("rejects a speed map that cannot render the final included event", () => {
    const project = createReplayProject(
      session([
        {id: "event-1", atMs: 0, kind: "user", text: "Start"},
        {id: "event-2", atMs: 7_750, kind: "assistant", markdown: "Finish"},
      ]),
    );

    const result = validateReplayProject({
      ...project,
      terminalHoldMs: 250,
      session: {...project.session, durationMs: 8_000},
      trim: {startMs: 0, endMs: 8_000},
      segments: [
        {
          id: "segment-1",
          sourceStartMs: 0,
          sourceEndMs: 8_000,
          speed: 8,
        },
      ],
    });

    expect(result).toMatchObject({
      ok: false,
      error: {code: "FINAL_EVENT_NOT_RENDERABLE"},
    });
  });

  it("accepts a one-frame replay when its event is visible on frame zero", () => {
    const project = createReplayProject(
      session([{id: "event-1", atMs: 0, kind: "user", text: "Start"}]),
    );

    const result = validateReplayProject({
      ...project,
      terminalHoldMs: 250,
      session: {...project.session, durationMs: 250},
      trim: {startMs: 0, endMs: 250},
      segments: [
        {
          id: "segment-1",
          sourceStartMs: 0,
          sourceEndMs: 250,
          speed: 8,
        },
      ],
    });

    expect(result.ok).toBe(true);
  });

  it("accepts a trim contained entirely in the final event hold", () => {
    const result = validateReplayProject({
      ...contractFixture,
      trim: {startMs: 6_500, endMs: 7_500},
      segments: [
        {
          id: "segment-1",
          sourceStartMs: 6_500,
          sourceEndMs: 7_500,
          speed: 1,
        },
      ],
    });

    expect(result.ok).toBe(true);
  });

  it("rejects a final event that inverse frame mapping does not reach", () => {
    const project = createReplayProject(
      session([
        {id: "event-1", atMs: 687, kind: "assistant", markdown: "Finish"},
      ]),
    );
    const result = validateReplayProject({
      ...project,
      terminalHoldMs: 250,
      session: {...project.session, durationMs: 937},
      trim: {startMs: 0, endMs: 688},
      segments: [
        {
          id: "segment-1",
          sourceStartMs: 0,
          sourceEndMs: 688,
          speed: 4.122,
        },
      ],
    });

    expect(result).toMatchObject({
      ok: false,
      error: {code: "FINAL_EVENT_NOT_RENDERABLE"},
    });
  });

  it("requires session duration to equal the configured terminal hold", () => {
    const project = createReplayProject(
      session([{id: "event-1", atMs: 0, kind: "user", text: "Start"}]),
    );

    const result = validateReplayProject({
      ...project,
      session: {...project.session, durationMs: 9_999},
      trim: {startMs: 0, endMs: 9_999},
      segments: [
        {
          id: "segment-1",
          sourceStartMs: 0,
          sourceEndMs: 9_999,
          speed: 1,
        },
      ],
    });

    expect(result).toMatchObject({
      ok: false,
      error: {code: "INVALID_SESSION_DURATION"},
    });
  });

  it.each([
    ["terminal hold", {terminalHoldMs: 250.5}],
    ["font size", {font: {...contractFixture.font, sizePx: 34.5}}],
    [
      "creation timestamp",
      {
        session: {
          ...contractFixture.session,
          createdAt: "August 24, 2026",
        },
      },
    ],
    [
      "unpaired surrogate",
      {
        session: {
          ...contractFixture.session,
          title: "\uD800",
        },
      },
    ],
  ])("rejects a project with an invalid %s", (_name, override) => {
    expect(
      validateReplayProject({...contractFixture, ...override}).ok,
    ).toBe(false);
  });

  it("uses Unicode scalar values for cross-language string limits", () => {
    const title = "😀".repeat(300);
    const result = validateReplayProject({
      ...contractFixture,
      session: {...contractFixture.session, title},
    });

    expect(result).toMatchObject({ok: true, value: {session: {title}}});
  });
});
