// @vitest-environment node

import {describe, expect, it} from "vitest";
import {
  createReplayProject,
  validateReplayProject,
  DEFAULT_TERMINAL_HOLD_MS,
  type NormalizedSessionV1,
} from "./index";

/**
 * Contract test: a Codex adapter NormalizedSessionV1 must be accepted by the
 * TypeScript createReplayProject factory and produce a valid ReplayProjectV1.
 */
describe("Codex adapter contract", () => {
  function codexSession(): NormalizedSessionV1 {
    return {
      schemaVersion: 1,
      id: "sess-codex-001",
      source: "codex",
      sourceVersion: "0.45.0",
      title: "sess-codex-001",
      createdAt: "2026-08-24T08:00:00.000Z",
      cwd: "C:\\code\\project",
      relationships: [
        {kind: "parent", sessionId: "parent-thread-001"},
      ],
      events: [
        {id: "e-user-001", atMs: 0, kind: "user", text: "Write a hello world program"},
        {id: "e-asst-001", atMs: 1000, kind: "assistant", markdown: "Sure, here is a hello world program."},
        {id: "e-tool-001", atMs: 2000, kind: "tool", name: "echo hello", status: "succeeded", summary: "Shell: echo hello"},
        {id: "e-tool-002", atMs: 3000, kind: "tool", name: "read_file", status: "running", summary: "Call: read_file"},
        {id: "e-fc-001", atMs: 5000, kind: "file-change", path: "hello.py", summary: "created: +1 -0"},
      ],
      durationMs: 5000 + DEFAULT_TERMINAL_HOLD_MS,
      diagnostics: [],
      unknownRecordCount: 0,
    };
  }

  it("accepts a Codex NormalizedSessionV1 and produces a valid ReplayProjectV1", () => {
    const session = codexSession();
    const project = createReplayProject(session);

    expect(project.schemaVersion).toBe(1);
    expect(project.session.source).toBe("codex");
    expect(project.session.id).toBe("sess-codex-001");
    expect(project.session.sourceVersion).toBe("0.45.0");
    expect(project.session.createdAt).toBe("2026-08-24T08:00:00.000Z");
    expect(project.session.relationships).toHaveLength(1);
    expect(project.session.events).toHaveLength(5);

    const validated = validateReplayProject(project);
    expect(validated.ok).toBe(true);
  });

  it("accepts a Codex session with unknown events", () => {
    const session: NormalizedSessionV1 = {
      ...codexSession(),
      events: [
        {id: "e-user-001", atMs: 0, kind: "user", text: "hello"},
        {id: "e-unknown-001", atMs: 1000, kind: "unknown", sourceType: "world_state"},
      ],
      durationMs: 1000 + DEFAULT_TERMINAL_HOLD_MS,
      unknownRecordCount: 1,
    };

    const project = createReplayProject(session);
    expect(project.session.unknownRecordCount).toBe(1);

    const validated = validateReplayProject(project);
    expect(validated.ok).toBe(true);
  });

  it("accepts a Codex session with file-change events", () => {
    const session: NormalizedSessionV1 = {
      ...codexSession(),
      events: [
        {id: "e-fc-001", atMs: 0, kind: "file-change", path: "src/main.rs", summary: "modified: +5 -2"},
      ],
      durationMs: DEFAULT_TERMINAL_HOLD_MS,
    };

    const project = createReplayProject(session);
    expect(project.session.events[0]!.kind).toBe("file-change");

    const validated = validateReplayProject(project);
    expect(validated.ok).toBe(true);
  });

  it("serialized camelCase shape round-trips through validateReplayProject", () => {
    const session = codexSession();
    const project = createReplayProject(session);
    const json = JSON.parse(JSON.stringify(project)) as unknown;

    const validated = validateReplayProject(json);
    expect(validated.ok).toBe(true);
  });
});
