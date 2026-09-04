import type {
  FontSpec,
  NormalizedSessionV1,
  NormalizedSessionValidationResult,
  ReplayEvent,
  ReplayProjectV1,
  SessionRelationship,
  SourceDiagnostic,
  SpeedSegment,
  ThemeSpec,
  ValidationError,
  ValidationResult,
} from "./types";

export const DEFAULT_TERMINAL_HOLD_MS = 1_500;
export const MAX_EVENT_COUNT = 100_000;
export const MAX_SEGMENT_COUNT = 10_000;
export const MAX_SOURCE_DURATION_MS = 7 * 24 * 60 * 60 * 1_000;
export const MAX_OUTPUT_DURATION_MS = 2 * 60 * 60 * 1_000;
export const MAX_FRAME_COUNT = 216_000;
export const MIN_SPEED = 0.25;
export const MAX_SPEED = 8;

const MAX_TEXT_LENGTH = 2_000_000;
const MAX_DIAGNOSTIC_COUNT = 1_000;
const MAX_RELATIONSHIP_COUNT = 100;

const DEFAULT_THEME: ThemeSpec = {
  background: "#101211",
  surface: "#171A18",
  text: "#F5F5F4",
  muted: "#9B9E9C",
  accent: "#D6AA68",
  success: "#A8D5BD",
  error: "#E59A91",
};

type ParseResult<T> =
  | Readonly<{ok: true; value: T}>
  | Readonly<{ok: false; error: ValidationError}>;

export function createReplayProject(
  inputSession: NormalizedSessionV1,
): ReplayProjectV1 {
  const parsedSession = parseSession(inputSession);
  if (!parsedSession.ok) {
    throw new Error(parsedSession.error.message);
  }

  const finalEvent = parsedSession.value.events.at(-1);
  if (finalEvent === undefined) {
    throw new Error("Session has no replayable events");
  }

  const endMs = finalEvent.atMs + DEFAULT_TERMINAL_HOLD_MS;
  if (endMs > MAX_SOURCE_DURATION_MS) {
    throw new Error("Session duration is outside the supported range");
  }

  const startMs = Math.max(0, endMs - MAX_OUTPUT_DURATION_MS);
  const candidate: ReplayProjectV1 = {
    schemaVersion: 1,
    session: {...parsedSession.value, durationMs: endMs},
    trim: {startMs, endMs},
    segments: [
      {
        id: "segment-1",
        sourceStartMs: startMs,
        sourceEndMs: endMs,
        speed: 1,
      },
    ],
    theme: DEFAULT_THEME,
    font: {
      family: "JetBrains Mono",
      sizePx: 34,
      lineHeight: 1.5,
    },
    terminalHoldMs: DEFAULT_TERMINAL_HOLD_MS,
    fps: 30,
    width: 1920,
    height: 1080,
  };

  const validated = validateReplayProject(candidate);
  if (!validated.ok) {
    throw new Error(validated.error.message);
  }
  return validated.value;
}

export function validateNormalizedSession(
  input: unknown,
): NormalizedSessionValidationResult {
  return parseSession(input);
}

export function validateReplayProject(input: unknown): ValidationResult {
  if (!isRecord(input) || input.schemaVersion !== 1) {
    return invalid("UNSUPPORTED_PROJECT_VERSION", "Project version is not supported");
  }

  const session = parseSession(input.session);
  if (!session.ok) {
    return session;
  }

  const terminalHoldMs = input.terminalHoldMs;
  if (
    input.fps !== 30 ||
    input.width !== 1920 ||
    input.height !== 1080 ||
    !Number.isSafeInteger(terminalHoldMs) ||
    !isFiniteBetween(terminalHoldMs, 250, 10_000)
  ) {
    return invalid("INVALID_RENDER_SETTINGS", "Render settings are invalid");
  }

  const finalSessionEvent = session.value.events.at(-1);
  if (
    finalSessionEvent === undefined ||
    session.value.durationMs !==
      finalSessionEvent.atMs + terminalHoldMs
  ) {
    return invalid(
      "INVALID_SESSION_DURATION",
      "Session duration must include the terminal hold",
    );
  }

  if (!isRecord(input.trim)) {
    return invalid("INVALID_TRIM", "Trim range is invalid");
  }
  const {startMs, endMs} = input.trim;
  if (
    !isSafeTime(startMs) ||
    !isSafeTime(endMs) ||
    startMs >= endMs ||
    endMs > session.value.durationMs ||
    endMs > MAX_SOURCE_DURATION_MS
  ) {
    return invalid("INVALID_TRIM", "Trim range is invalid");
  }

  const segments = parseSegments(input.segments, startMs, endMs);
  if (!segments.ok) {
    return segments;
  }

  const outputDuration = calculateOutputDuration(segments.value);
  if (
    !Number.isFinite(outputDuration) ||
    outputDuration > MAX_OUTPUT_DURATION_MS
  ) {
    return invalid(
      "OUTPUT_DURATION_LIMIT_EXCEEDED",
      "Output duration exceeds two hours",
    );
  }

  const frames = Math.ceil((outputDuration / 1_000) * 30);
  if (!Number.isSafeInteger(frames) || frames < 1 || frames > MAX_FRAME_COUNT) {
    return invalid("FRAME_LIMIT_EXCEEDED", "Frame count is outside the supported range");
  }

  const finalIncludedEvent = session.value.events.findLast(
    (event) => event.atMs < endMs,
  );
  if (finalIncludedEvent === undefined) {
    return invalid("EMPTY_TRIM", "Trim range contains no replayable events");
  }
  const finalFrameOutputMs = ((frames - 1) / 30) * 1_000;
  const finalFrameSourceMs = sourceTimeAtOutputMs(
    segments.value,
    finalFrameOutputMs,
  );
  if (finalFrameSourceMs < Math.max(finalIncludedEvent.atMs, startMs)) {
    return invalid(
      "FINAL_EVENT_NOT_RENDERABLE",
      "The final event must remain visible for at least one frame",
    );
  }

  const theme = parseTheme(input.theme);
  if (!theme.ok) {
    return theme;
  }
  const font = parseFont(input.font);
  if (!font.ok) {
    return font;
  }

  return {
    ok: true,
    value: {
      schemaVersion: 1,
      session: session.value,
      trim: {startMs, endMs},
      segments: segments.value,
      theme: theme.value,
      font: font.value,
      terminalHoldMs,
      fps: 30,
      width: 1920,
      height: 1080,
    },
  };
}

function parseSession(input: unknown): ParseResult<NormalizedSessionV1> {
  if (
    !isRecord(input) ||
    input.schemaVersion !== 1 ||
    !isBoundedString(input.id, 1, 256) ||
    !isSessionSource(input.source) ||
    !isNullableBoundedString(input.sourceVersion, 128) ||
    !isBoundedString(input.title, 1, 512) ||
    !isNullableIsoDate(input.createdAt) ||
    !isNullableBoundedString(input.cwd, 32_768) ||
    !Array.isArray(input.events)
  ) {
    return invalidParsed("INVALID_SESSION", "Project session is invalid");
  }

  if (input.events.length === 0) {
    return invalidParsed("EMPTY_SESSION", "Session has no replayable events");
  }
  if (input.events.length > MAX_EVENT_COUNT) {
    return invalidParsed(
      "EVENT_LIMIT_EXCEEDED",
      `Projects may contain at most ${MAX_EVENT_COUNT} events`,
    );
  }

  const events: ReplayEvent[] = [];
  const eventIds = new Set<string>();
  let previousAtMs = -1;
  for (const eventInput of input.events) {
    const event = parseEvent(eventInput);
    if (!event.ok) {
      return event;
    }
    if (event.value.atMs < previousAtMs) {
      return invalidParsed(
        "INVALID_EVENT_TIMELINE",
        "Event times must be monotonic",
      );
    }
    if (eventIds.has(event.value.id)) {
      return invalidParsed("DUPLICATE_EVENT_ID", "Event IDs must be unique");
    }
    eventIds.add(event.value.id);
    previousAtMs = event.value.atMs;
    events.push(event.value);
  }

  if (
    !isSafeTime(input.durationMs) ||
    input.durationMs > MAX_SOURCE_DURATION_MS ||
    input.durationMs < previousAtMs
  ) {
    return invalidParsed("INVALID_SESSION_DURATION", "Session duration is invalid");
  }

  const relationships = parseRelationships(input.relationships);
  if (!relationships.ok) {
    return relationships;
  }
  const diagnostics = parseDiagnostics(input.diagnostics);
  if (!diagnostics.ok) {
    return diagnostics;
  }
  if (
    !isSafeTime(input.unknownRecordCount) ||
    input.unknownRecordCount > events.length
  ) {
    return invalidParsed(
      "INVALID_UNKNOWN_RECORD_COUNT",
      "Unknown record count is invalid",
    );
  }

  return {
    ok: true,
    value: {
      schemaVersion: 1,
      id: input.id,
      source: input.source,
      sourceVersion: input.sourceVersion,
      title: input.title,
      createdAt: input.createdAt,
      cwd: input.cwd,
      relationships: relationships.value,
      events,
      durationMs: input.durationMs,
      diagnostics: diagnostics.value,
      unknownRecordCount: input.unknownRecordCount,
    },
  };
}

function parseEvent(input: unknown): ParseResult<ReplayEvent> {
  if (
    !isRecord(input) ||
    !isBoundedString(input.id, 1, 256) ||
    !isSafeTime(input.atMs) ||
    input.atMs > MAX_SOURCE_DURATION_MS
  ) {
    return invalidParsed("INVALID_EVENT", "Replay event is invalid");
  }

  switch (input.kind) {
    case "user":
      return isBoundedString(input.text, 0, MAX_TEXT_LENGTH)
        ? {ok: true, value: {id: input.id, atMs: input.atMs, kind: "user", text: input.text}}
        : invalidParsed("INVALID_EVENT", "User event is invalid");
    case "assistant":
      return isBoundedString(input.markdown, 0, MAX_TEXT_LENGTH)
        ? {
            ok: true,
            value: {
              id: input.id,
              atMs: input.atMs,
              kind: "assistant",
              markdown: input.markdown,
            },
          }
        : invalidParsed("INVALID_EVENT", "Assistant event is invalid");
    case "tool":
      return isBoundedString(input.name, 1, 256) &&
        isBoundedString(input.summary, 0, MAX_TEXT_LENGTH) &&
        (input.status === "running" ||
          input.status === "succeeded" ||
          input.status === "failed")
        ? {
            ok: true,
            value: {
              id: input.id,
              atMs: input.atMs,
              kind: "tool",
              name: input.name,
              status: input.status,
              summary: input.summary,
            },
          }
        : invalidParsed("INVALID_EVENT", "Tool event is invalid");
    case "file-change":
      return isBoundedString(input.path, 1, 32_768) &&
        isBoundedString(input.summary, 0, MAX_TEXT_LENGTH)
        ? {
            ok: true,
            value: {
              id: input.id,
              atMs: input.atMs,
              kind: "file-change",
              path: input.path,
              summary: input.summary,
            },
          }
        : invalidParsed("INVALID_EVENT", "File change event is invalid");
    case "unknown":
      return isBoundedString(input.sourceType, 1, 256)
        ? {
            ok: true,
            value: {
              id: input.id,
              atMs: input.atMs,
              kind: "unknown",
              sourceType: input.sourceType,
            },
          }
        : invalidParsed("INVALID_EVENT", "Unknown event is invalid");
    default:
      return invalidParsed("INVALID_EVENT", "Replay event kind is invalid");
  }
}

function parseSegments(
  input: unknown,
  trimStartMs: number,
  trimEndMs: number,
): ParseResult<readonly SpeedSegment[]> {
  if (
    !Array.isArray(input) ||
    input.length === 0 ||
    input.length > MAX_SEGMENT_COUNT
  ) {
    return invalidParsed("INVALID_SEGMENTS", "Speed segment count is invalid");
  }

  const segments: SpeedSegment[] = [];
  const ids = new Set<string>();
  let expectedStart = trimStartMs;
  for (const candidate of input) {
    if (
      !isRecord(candidate) ||
      !isBoundedString(candidate.id, 1, 256) ||
      ids.has(candidate.id) ||
      !isSafeTime(candidate.sourceStartMs) ||
      !isSafeTime(candidate.sourceEndMs) ||
      !isFiniteBetween(candidate.speed, MIN_SPEED, MAX_SPEED) ||
      candidate.sourceStartMs !== expectedStart ||
      candidate.sourceStartMs >= candidate.sourceEndMs
    ) {
      return invalidParsed(
        "INVALID_SEGMENT_PARTITION",
        "Speed segments must exactly partition the trim range",
      );
    }
    ids.add(candidate.id);
    segments.push({
      id: candidate.id,
      sourceStartMs: candidate.sourceStartMs,
      sourceEndMs: candidate.sourceEndMs,
      speed: candidate.speed,
    });
    expectedStart = candidate.sourceEndMs;
  }

  return expectedStart === trimEndMs
    ? {ok: true, value: segments}
    : invalidParsed(
        "INVALID_SEGMENT_PARTITION",
        "Speed segments must exactly partition the trim range",
      );
}

function parseRelationships(input: unknown): ParseResult<readonly SessionRelationship[]> {
  if (!Array.isArray(input) || input.length > MAX_RELATIONSHIP_COUNT) {
    return invalidParsed("INVALID_RELATIONSHIPS", "Session relationships are invalid");
  }
  const relationships: SessionRelationship[] = [];
  for (const candidate of input) {
    if (
      !isRecord(candidate) ||
      (candidate.kind !== "parent" && candidate.kind !== "fork") ||
      !isBoundedString(candidate.sessionId, 1, 256)
    ) {
      return invalidParsed("INVALID_RELATIONSHIPS", "Session relationships are invalid");
    }
    relationships.push({kind: candidate.kind, sessionId: candidate.sessionId});
  }
  return {ok: true, value: relationships};
}

function parseDiagnostics(input: unknown): ParseResult<readonly SourceDiagnostic[]> {
  if (!Array.isArray(input) || input.length > MAX_DIAGNOSTIC_COUNT) {
    return invalidParsed("INVALID_DIAGNOSTICS", "Session diagnostics are invalid");
  }
  const diagnostics: SourceDiagnostic[] = [];
  for (const candidate of input) {
    if (
      !isRecord(candidate) ||
      !isBoundedString(candidate.code, 1, 128) ||
      !isBoundedString(candidate.message, 1, 4_096) ||
      (candidate.severity !== "info" &&
        candidate.severity !== "warning" &&
        candidate.severity !== "error")
    ) {
      return invalidParsed("INVALID_DIAGNOSTICS", "Session diagnostics are invalid");
    }
    diagnostics.push({
      code: candidate.code,
      severity: candidate.severity,
      message: candidate.message,
    });
  }
  return {ok: true, value: diagnostics};
}

function parseTheme(input: unknown): ParseResult<ThemeSpec> {
  if (
    !isRecord(input) ||
    !isHexColor(input.background) ||
    !isHexColor(input.surface) ||
    !isHexColor(input.text) ||
    !isHexColor(input.muted) ||
    !isHexColor(input.accent) ||
    !isHexColor(input.success) ||
    !isHexColor(input.error)
  ) {
    return invalidParsed("INVALID_THEME", "Theme colors are invalid");
  }
  return {
    ok: true,
    value: {
      background: input.background,
      surface: input.surface,
      text: input.text,
      muted: input.muted,
      accent: input.accent,
      success: input.success,
      error: input.error,
    },
  };
}

function parseFont(input: unknown): ParseResult<FontSpec> {
  if (
    !isRecord(input) ||
    input.family !== "JetBrains Mono" ||
    !Number.isSafeInteger(input.sizePx) ||
    !isFiniteBetween(input.sizePx, 12, 96) ||
    !isFiniteBetween(input.lineHeight, 1, 2.5)
  ) {
    return invalidParsed("INVALID_FONT", "Font settings are invalid");
  }
  return {
    ok: true,
    value: {
      family: input.family,
      sizePx: input.sizePx,
      lineHeight: input.lineHeight,
    },
  };
}

function calculateOutputDuration(segments: readonly SpeedSegment[]): number {
  return segments.reduce(
    (total, segment) =>
      total +
      (segment.sourceEndMs - segment.sourceStartMs) / segment.speed,
    0,
  );
}

function sourceTimeAtOutputMs(
  segments: readonly SpeedSegment[],
  outputMs: number,
): number {
  let outputCursor = 0;
  for (const segment of segments) {
    const segmentOutputDuration =
      (segment.sourceEndMs - segment.sourceStartMs) / segment.speed;
    const outputEnd = outputCursor + segmentOutputDuration;
    if (outputMs <= outputEnd) {
      return Math.min(
        segment.sourceEndMs,
        segment.sourceStartMs +
          (outputMs - outputCursor) * segment.speed,
      );
    }
    outputCursor = outputEnd;
  }
  return segments.at(-1)?.sourceEndMs ?? 0;
}

function isSessionSource(
  value: unknown,
): value is NormalizedSessionV1["source"] {
  return (
    value === "claude-code" ||
    value === "codex" ||
    value === "copilot-cli" ||
    value === "vscode-copilot"
  );
}

function isNullableIsoDate(value: unknown): value is string | null {
  return (
    value === null ||
    (typeof value === "string" &&
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value) &&
      Number.isFinite(Date.parse(value)) &&
      new Date(value).toISOString() === value)
  );
}

function isNullableBoundedString(
  value: unknown,
  maximum: number,
): value is string | null {
  return value === null || isBoundedString(value, 0, maximum);
}

function isBoundedString(
  value: unknown,
  minimum: number,
  maximum: number,
): value is string {
  if (typeof value !== "string") {
    return false;
  }
  let length = 0;
  for (const character of value) {
    const codePoint = character.charCodeAt(0);
    if (
      character === "\0" ||
      (character.length === 1 && codePoint >= 0xd800 && codePoint <= 0xdfff) ||
      ++length > maximum
    ) {
      return false;
    }
  }
  return length >= minimum;
}

function isHexColor(value: unknown): value is string {
  return typeof value === "string" && /^#[0-9A-Fa-f]{6}$/.test(value);
}

function isRecord(input: unknown): input is Record<string, unknown> {
  return typeof input === "object" && input !== null && !Array.isArray(input);
}

function isSafeTime(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isFiniteBetween(
  value: unknown,
  minimum: number,
  maximum: number,
): value is number {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    value >= minimum &&
    value <= maximum
  );
}

function invalid(
  code: string,
  message: string,
): Extract<ValidationResult, {ok: false}> {
  return {ok: false, error: {code, message}};
}

function invalidParsed<T>(code: string, message: string): ParseResult<T> {
  return {ok: false, error: {code, message}};
}
