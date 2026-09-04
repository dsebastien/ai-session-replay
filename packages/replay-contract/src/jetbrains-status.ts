import type {LibraryContractValidationResult} from "./indexed-library";

export type JetBrainsPluginStatusV1 = "not-detected" | "detected" | "unavailable";
export type JetBrainsNativeTranscriptStatusV1 = "unsupported";
export type JetBrainsCopilotCliStatusV1 =
  | "not-detected"
  | "available-separately"
  | "jetbrains-attributed"
  | "unavailable";

export interface JetBrainsCopilotStatusV1 {
  readonly schemaVersion: 1;
  readonly pluginStatus: JetBrainsPluginStatusV1;
  readonly nativeTranscriptStatus: JetBrainsNativeTranscriptStatusV1;
  readonly copilotCliStatus: JetBrainsCopilotCliStatusV1;
}

export function validateJetBrainsCopilotStatus(
  input: unknown,
): LibraryContractValidationResult<JetBrainsCopilotStatusV1> {
  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    return invalid();
  }
  const record = input as Record<string, unknown>;
  const expectedKeys = new Set([
    "schemaVersion",
    "pluginStatus",
    "nativeTranscriptStatus",
    "copilotCliStatus",
  ]);
  if (
    Object.keys(record).some((key) => !expectedKeys.has(key)) ||
    record.schemaVersion !== 1 ||
    !isPluginStatus(record.pluginStatus) ||
    record.nativeTranscriptStatus !== "unsupported" ||
    !isCopilotCliStatus(record.copilotCliStatus)
  ) {
    return invalid();
  }
  return {
    ok: true,
    value: {
      schemaVersion: 1,
      pluginStatus: record.pluginStatus,
      nativeTranscriptStatus: "unsupported",
      copilotCliStatus: record.copilotCliStatus,
    },
  };
}

function isPluginStatus(value: unknown): value is JetBrainsPluginStatusV1 {
  return value === "not-detected" || value === "detected" || value === "unavailable";
}

function isCopilotCliStatus(value: unknown): value is JetBrainsCopilotCliStatusV1 {
  return value === "not-detected" ||
    value === "available-separately" ||
    value === "jetbrains-attributed" ||
    value === "unavailable";
}

function invalid(): LibraryContractValidationResult<never> {
  return {
    ok: false,
    error: {
      code: "INVALID_JETBRAINS_STATUS",
      message: "JetBrains Copilot status is invalid",
    },
  };
}
