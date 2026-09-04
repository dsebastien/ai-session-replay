// @vitest-environment node

import {describe, expect, it} from "vitest";
import type {JetBrainsCopilotStatusV1} from "../../../packages/replay-contract/src";
import {jetBrainsStatusLabel} from "./SourceSidebar";

const status = (
  pluginStatus: JetBrainsCopilotStatusV1["pluginStatus"],
  copilotCliStatus: JetBrainsCopilotStatusV1["copilotCliStatus"],
): JetBrainsCopilotStatusV1 => ({
  schemaVersion: 1,
  pluginStatus,
  nativeTranscriptStatus: "unsupported",
  copilotCliStatus,
});

describe("JetBrains Copilot sidebar status", () => {
  it.each([
    [null, "Checking integration…"],
    [status("not-detected", "not-detected"), "Not detected"],
    [status("detected", "not-detected"), "Plugin found"],
    [status("not-detected", "available-separately"), "Copilot CLI available separately"],
    [status("detected", "available-separately"), "Plugin found · Copilot CLI separate"],
    [status("detected", "jetbrains-attributed"), "JetBrains-attributed CLI session found"],
    [status("detected", "unavailable"), "Plugin found · CLI detection unavailable"],
    [status("unavailable", "available-separately"), "Copilot CLI separate · plugin detection unavailable"],
  ] as const)("renders %s honestly", (value, expected) => {
    expect(jetBrainsStatusLabel(value)).toBe(expected);
  });
});
