// @vitest-environment node

import {describe, expect, it} from "vitest";
import fixture from "../../../tests/fixtures/jetbrains-status-v1.json";
import {validateJetBrainsCopilotStatus} from "./jetbrains-status";

describe("JetBrains Copilot status contract", () => {
  it.each(fixture.valid)("accepts a path-free $pluginStatus/$copilotCliStatus status", (value) => {
    expect(validateJetBrainsCopilotStatus(value)).toEqual({ok: true, value});
    expect(JSON.stringify(value)).not.toMatch(/[A-Z]:\\|\\\\/i);
  });

  it.each([
    null,
    {},
    {...fixture.valid[0], schemaVersion: 2},
    {...fixture.valid[0], pluginStatus: "installed"},
    {...fixture.valid[0], nativeTranscriptStatus: "available"},
    {...fixture.valid[0], copilotCliStatus: "attributed"},
    {...fixture.valid[0], path: "C:\\private"},
  ])("rejects malformed or expanded status payloads", (value) => {
    expect(validateJetBrainsCopilotStatus(value).ok).toBe(false);
  });
});
