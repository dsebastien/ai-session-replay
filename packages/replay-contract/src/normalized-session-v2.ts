import {MAX_EVENT_COUNT, MAX_SOURCE_DURATION_MS, validateNormalizedSession} from "./schema";
import type {
  NormalizedSessionV1,
  SessionRelationship,
  SessionSource,
  SourceDiagnostic,
  ValidationError,
} from "./types";

export const NORMALIZED_SESSION_SCHEMA_VERSION = 2 as const;

const MAX_TEXT_LENGTH = 2_000_000;
const MAX_DIAGNOSTIC_COUNT = 1_000;
const MAX_RELATIONSHIP_COUNT = 100;
const MAX_ENTRY_KEY_LENGTH = 256;

export type NormalizedToolDetailV2 =
  | Readonly<{availability: "unavailable"}>
  | Readonly<{
      availability: "available";
      arguments: string | null;
      result: string | null;
    }>;

interface NormalizedEntryBaseV2 {
  readonly entryKey: string;
  readonly atMs: number;
}

export type NormalizedEntryV2 =
  | Readonly<NormalizedEntryBaseV2 & {kind: "user"; text: string}>
  | Readonly<NormalizedEntryBaseV2 & {kind: "assistant"; markdown: string}>
  | Readonly<NormalizedEntryBaseV2 & {kind: "reasoning"; text: string}>
  | Readonly<
      NormalizedEntryBaseV2 & {
        kind: "tool-call";
        name: string;
        status: "pending" | "running" | "succeeded" | "failed";
        summary: string;
        detail: NormalizedToolDetailV2;
      }
    >
  | Readonly<
      NormalizedEntryBaseV2 & {
        kind: "file-change";
        displayPath: string;
        summary: string;
      }
    >
  | Readonly<NormalizedEntryBaseV2 & {kind: "unknown"; sourceType: string}>;

export interface NormalizedContentAvailabilityV2 {
  readonly reasoning: "available" | "unavailable";
  readonly toolDetails: "available" | "partial" | "unavailable";
}

export interface NormalizedSessionV2 {
  readonly schemaVersion: 2;
  readonly id: string;
  readonly source: SessionSource;
  readonly sourceVersion: string | null;
  readonly title: string;
  readonly createdAt: string | null;
  readonly cwd: string | null;
  readonly relationships: readonly SessionRelationship[];
  readonly entries: readonly NormalizedEntryV2[];
  readonly durationMs: number;
  readonly diagnostics: readonly SourceDiagnostic[];
  readonly unknownRecordCount: number;
  readonly contentAvailability: NormalizedContentAvailabilityV2;
}

export type NormalizedSessionV2ValidationResult =
  | Readonly<{ok: true; value: NormalizedSessionV2}>
  | Readonly<{ok: false; error: ValidationError}>;

type ParseResult<T> =
  | Readonly<{ok: true; value: T}>
  | Readonly<{ok: false; error: ValidationError}>;

type JsonRecord = Readonly<Record<string, unknown>>;

export function validateNormalizedSessionV2(
  input: unknown,
): NormalizedSessionV2ValidationResult {
  const record = parseRecord(
    input,
    [
      "schemaVersion",
      "id",
      "source",
      "sourceVersion",
      "title",
      "createdAt",
      "cwd",
      "relationships",
      "entries",
      "durationMs",
      "diagnostics",
      "unknownRecordCount",
      "contentAvailability",
    ],
    "INVALID_SESSION",
  );
  if (!record.ok) return record;
  if (record.value.schemaVersion !== NORMALIZED_SESSION_SCHEMA_VERSION) {
    return invalid("UNSUPPORTED_SESSION_VERSION", "Normalized session version is not supported");
  }
  if (
    !isBoundedString(record.value.id, 1, 256) ||
    !isSessionSource(record.value.source) ||
    !isNullableBoundedString(record.value.sourceVersion, 128) ||
    !isBoundedString(record.value.title, 1, 512) ||
    !isNullableIsoDate(record.value.createdAt) ||
    !isNullableBoundedString(record.value.cwd, 32_768)
  ) {
    return invalid("INVALID_SESSION", "Normalized session metadata is invalid");
  }

  const relationships = parseRelationships(record.value.relationships);
  if (!relationships.ok) return relationships;
  const diagnostics = parseDiagnostics(record.value.diagnostics);
  if (!diagnostics.ok) return diagnostics;
  const availability = parseAvailability(record.value.contentAvailability);
  if (!availability.ok) return availability;
  const entries = parseEntries(record.value.entries);
  if (!entries.ok) return entries;

  const lastAtMs = entries.value.at(-1)?.atMs ?? 0;
  if (
    !isSafeIntegerBetween(record.value.durationMs, lastAtMs, MAX_SOURCE_DURATION_MS)
  ) {
    return invalid("INVALID_SESSION_DURATION", "Normalized session duration is invalid");
  }
  const unknownCount = entries.value.filter(({kind}) => kind === "unknown").length;
  if (record.value.unknownRecordCount !== unknownCount) {
    return invalid(
      "INVALID_UNKNOWN_RECORD_COUNT",
      "Unknown record count must match the normalized entries",
    );
  }
  const availabilityError = validateAvailability(entries.value, availability.value);
  if (availabilityError !== null) return availabilityError;

  return valid({
    schemaVersion: NORMALIZED_SESSION_SCHEMA_VERSION,
    id: record.value.id,
    source: record.value.source,
    sourceVersion: record.value.sourceVersion,
    title: record.value.title,
    createdAt: record.value.createdAt,
    cwd: record.value.cwd,
    relationships: relationships.value,
    entries: entries.value,
    durationMs: record.value.durationMs,
    diagnostics: diagnostics.value,
    unknownRecordCount: unknownCount,
    contentAvailability: availability.value,
  });
}

export function migrateNormalizedSessionV1(
  input: unknown,
): NormalizedSessionV2ValidationResult {
  const legacy = validateNormalizedSession(input);
  if (!legacy.ok) return legacy;

  const candidate: NormalizedSessionV2 = {
    schemaVersion: NORMALIZED_SESSION_SCHEMA_VERSION,
    id: legacy.value.id,
    source: legacy.value.source,
    sourceVersion: legacy.value.sourceVersion,
    title: legacy.value.title,
    createdAt: legacy.value.createdAt,
    cwd: legacy.value.cwd,
    relationships: legacy.value.relationships,
    entries: legacy.value.events.map((event) => {
      const entryKey = migrateLegacyEntryKey(event.id);
      switch (event.kind) {
        case "user":
          return {entryKey, atMs: event.atMs, kind: "user", text: event.text};
        case "assistant":
          return {entryKey, atMs: event.atMs, kind: "assistant", markdown: event.markdown};
        case "tool":
          return {
            entryKey,
            atMs: event.atMs,
            kind: "tool-call",
            name: event.name,
            status: event.status,
            summary: event.summary,
            detail: {availability: "unavailable"},
          };
        case "file-change":
          return {
            entryKey,
            atMs: event.atMs,
            kind: "file-change",
            displayPath: event.path,
            summary: event.summary,
          };
        case "unknown":
          return {entryKey, atMs: event.atMs, kind: "unknown", sourceType: event.sourceType};
      }
    }),
    durationMs: legacy.value.durationMs,
    diagnostics: legacy.value.diagnostics,
    unknownRecordCount: legacy.value.unknownRecordCount,
    contentAvailability: {reasoning: "unavailable", toolDetails: "unavailable"},
  };
  return validateNormalizedSessionV2(candidate);
}

function parseEntries(input: unknown): ParseResult<readonly NormalizedEntryV2[]> {
  if (!Array.isArray(input) || input.length === 0) {
    return invalid("EMPTY_SESSION", "Normalized session has no entries");
  }
  if (input.length > MAX_EVENT_COUNT) {
    return invalid("ENTRY_LIMIT_EXCEEDED", `Sessions may contain at most ${MAX_EVENT_COUNT} entries`);
  }

  const entries: NormalizedEntryV2[] = [];
  const keys = new Set<string>();
  let previousAtMs = -1;
  for (const candidate of input) {
    const entry = parseEntry(candidate);
    if (!entry.ok) return entry;
    if (keys.has(entry.value.entryKey)) {
      return invalid("DUPLICATE_ENTRY_KEY", "Normalized entry keys must be unique");
    }
    if (entry.value.atMs < previousAtMs) {
      return invalid("INVALID_ENTRY_TIMELINE", "Normalized entry times must be monotonic");
    }
    keys.add(entry.value.entryKey);
    previousAtMs = entry.value.atMs;
    entries.push(entry.value);
  }
  return valid(entries);
}

function parseEntry(input: unknown): ParseResult<NormalizedEntryV2> {
  if (!isRecord(input)) return invalid("INVALID_ENTRY", "Normalized entry is invalid");
  if (!isEntryKey(input.entryKey)) {
    return invalid("INVALID_ENTRY_KEY", "Normalized entry key is invalid");
  }
  if (!isSafeIntegerBetween(input.atMs, 0, MAX_SOURCE_DURATION_MS)) {
    return invalid("INVALID_ENTRY", "Normalized entry timestamp is invalid");
  }
  const base = {entryKey: input.entryKey, atMs: input.atMs};
  switch (input.kind) {
    case "user":
      if (!hasOnlyKeys(input, ["entryKey", "atMs", "kind", "text"])) return unknownField();
      return isBoundedString(input.text, 0, MAX_TEXT_LENGTH)
        ? valid({...base, kind: "user", text: input.text})
        : invalid("INVALID_ENTRY", "User entry is invalid");
    case "assistant":
      if (!hasOnlyKeys(input, ["entryKey", "atMs", "kind", "markdown"])) return unknownField();
      return isBoundedString(input.markdown, 0, MAX_TEXT_LENGTH)
        ? valid({...base, kind: "assistant", markdown: input.markdown})
        : invalid("INVALID_ENTRY", "Assistant entry is invalid");
    case "reasoning":
      if (!hasOnlyKeys(input, ["entryKey", "atMs", "kind", "text"])) return unknownField();
      return isBoundedString(input.text, 0, MAX_TEXT_LENGTH)
        ? valid({...base, kind: "reasoning", text: input.text})
        : invalid("INVALID_ENTRY", "Reasoning entry is invalid");
    case "tool-call": {
      if (!hasOnlyKeys(input, ["entryKey", "atMs", "kind", "name", "status", "summary", "detail"])) return unknownField();
      const detail = parseToolDetail(input.detail);
      if (!detail.ok) return detail;
      if (
        !isBoundedString(input.name, 1, 256) ||
        !isToolStatus(input.status) ||
        !isBoundedString(input.summary, 0, MAX_TEXT_LENGTH)
      ) {
        return invalid("INVALID_ENTRY", "Tool-call entry is invalid");
      }
      return valid({...base, kind: "tool-call", name: input.name, status: input.status, summary: input.summary, detail: detail.value});
    }
    case "file-change":
      if (!hasOnlyKeys(input, ["entryKey", "atMs", "kind", "displayPath", "summary"])) return unknownField();
      return isBoundedString(input.displayPath, 1, 32_768) && isBoundedString(input.summary, 0, MAX_TEXT_LENGTH)
        ? valid({...base, kind: "file-change", displayPath: input.displayPath, summary: input.summary})
        : invalid("INVALID_ENTRY", "File-change entry is invalid");
    case "unknown":
      if (!hasOnlyKeys(input, ["entryKey", "atMs", "kind", "sourceType"])) return unknownField();
      return isBoundedString(input.sourceType, 1, 256)
        ? valid({...base, kind: "unknown", sourceType: input.sourceType})
        : invalid("INVALID_ENTRY", "Unknown entry is invalid");
    default:
      return invalid("INVALID_ENTRY_KIND", "Normalized entry kind is invalid");
  }
}

function parseToolDetail(input: unknown): ParseResult<NormalizedToolDetailV2> {
  if (!isRecord(input) || input.availability === undefined) {
    return invalid("INVALID_TOOL_DETAIL", "Tool detail is invalid");
  }
  if (input.availability === "unavailable") {
    return hasOnlyKeys(input, ["availability"])
      ? valid({availability: "unavailable"})
      : unknownField();
  }
  if (input.availability !== "available" || !hasOnlyKeys(input, ["availability", "arguments", "result"])) {
    return invalid("INVALID_TOOL_DETAIL", "Tool detail is invalid");
  }
  if (
    !isNullableBoundedString(input.arguments, MAX_TEXT_LENGTH) ||
    !isNullableBoundedString(input.result, MAX_TEXT_LENGTH) ||
    (input.arguments === null && input.result === null)
  ) {
    return invalid("INVALID_TOOL_DETAIL", "Available tool detail must contain arguments or result");
  }
  return valid({availability: "available", arguments: input.arguments, result: input.result});
}

function parseAvailability(input: unknown): ParseResult<NormalizedContentAvailabilityV2> {
  const record = parseRecord(input, ["reasoning", "toolDetails"], "INVALID_CONTENT_AVAILABILITY");
  if (!record.ok) return record;
  if (
    (record.value.reasoning !== "available" && record.value.reasoning !== "unavailable") ||
    (record.value.toolDetails !== "available" &&
      record.value.toolDetails !== "partial" &&
      record.value.toolDetails !== "unavailable")
  ) {
    return invalid("INVALID_CONTENT_AVAILABILITY", "Content availability is invalid");
  }
  return valid({reasoning: record.value.reasoning, toolDetails: record.value.toolDetails});
}

function validateAvailability(
  entries: readonly NormalizedEntryV2[],
  availability: NormalizedContentAvailabilityV2,
): ParseResult<never> | null {
  if (
    availability.reasoning === "unavailable" &&
    entries.some(({kind}) => kind === "reasoning")
  ) {
    return invalid("CONTENT_AVAILABILITY_MISMATCH", "Reasoning availability contradicts the entries");
  }
  const toolDetails = entries
    .filter((entry): entry is Extract<NormalizedEntryV2, {kind: "tool-call"}> => entry.kind === "tool-call")
    .map(({detail}) => detail.availability);
  const hasAvailable = toolDetails.includes("available");
  const hasUnavailable = toolDetails.includes("unavailable");
  if (
    (availability.toolDetails === "unavailable" && hasAvailable) ||
    (availability.toolDetails === "available" && hasUnavailable) ||
    (availability.toolDetails === "partial" && (!hasAvailable || !hasUnavailable))
  ) {
    return invalid("CONTENT_AVAILABILITY_MISMATCH", "Tool-detail availability contradicts the entries");
  }
  return null;
}

function parseRelationships(input: unknown): ParseResult<readonly SessionRelationship[]> {
  if (!Array.isArray(input) || input.length > MAX_RELATIONSHIP_COUNT) {
    return invalid("INVALID_RELATIONSHIPS", "Session relationships are invalid");
  }
  const relationships: SessionRelationship[] = [];
  for (const candidate of input) {
    const record = parseRecord(candidate, ["kind", "sessionId"], "INVALID_RELATIONSHIPS");
    if (!record.ok) return record;
    if (
      (record.value.kind !== "parent" && record.value.kind !== "fork") ||
      !isBoundedString(record.value.sessionId, 1, 256)
    ) {
      return invalid("INVALID_RELATIONSHIPS", "Session relationships are invalid");
    }
    relationships.push({kind: record.value.kind, sessionId: record.value.sessionId});
  }
  return valid(relationships);
}

function parseDiagnostics(input: unknown): ParseResult<readonly SourceDiagnostic[]> {
  if (!Array.isArray(input) || input.length > MAX_DIAGNOSTIC_COUNT) {
    return invalid("INVALID_DIAGNOSTICS", "Session diagnostics are invalid");
  }
  const diagnostics: SourceDiagnostic[] = [];
  for (const candidate of input) {
    const record = parseRecord(candidate, ["code", "severity", "message"], "INVALID_DIAGNOSTICS");
    if (!record.ok) return record;
    if (
      !isBoundedString(record.value.code, 1, 128) ||
      !isBoundedString(record.value.message, 1, 4_096) ||
      (record.value.severity !== "info" && record.value.severity !== "warning" && record.value.severity !== "error")
    ) {
      return invalid("INVALID_DIAGNOSTICS", "Session diagnostics are invalid");
    }
    diagnostics.push({
      code: record.value.code,
      severity: record.value.severity,
      message: record.value.message,
    });
  }
  return valid(diagnostics);
}

function migrateLegacyEntryKey(id: string): string {
  if (isEntryKey(id)) return id;
  const bytes = new TextEncoder().encode(id);
  return `legacy-${fnv1a32(bytes, 0x811c9dc5)}${fnv1a32(bytes, 0x9e3779b9)}`;
}

function fnv1a32(bytes: Uint8Array, seed: number): string {
  let hash = seed;
  for (const byte of bytes) hash = Math.imul((hash ^ byte) >>> 0, 0x01000193) >>> 0;
  return hash.toString(16).padStart(8, "0");
}

function parseRecord(input: unknown, keys: readonly string[], code: string): ParseResult<JsonRecord> {
  if (!isRecord(input)) return invalid(code, "Contract object is invalid");
  return hasOnlyKeys(input, keys) ? valid(input) : unknownField();
}

function isRecord(input: unknown): input is JsonRecord {
  return typeof input === "object" && input !== null && !Array.isArray(input);
}

function hasOnlyKeys(input: JsonRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(input).every((key) => allowed.has(key));
}

function isSessionSource(value: unknown): value is SessionSource {
  return value === "claude-code" || value === "codex" || value === "copilot-cli" || value === "vscode-copilot";
}

function isToolStatus(value: unknown): value is Extract<NormalizedEntryV2, {kind: "tool-call"}>["status"] {
  return value === "pending" || value === "running" || value === "succeeded" || value === "failed";
}

function isEntryKey(value: unknown): value is string {
  return typeof value === "string" && value.length <= MAX_ENTRY_KEY_LENGTH && /^[A-Za-z0-9_-]+$/.test(value);
}

function isNullableIsoDate(value: unknown): value is string | null {
  return value === null || (
    typeof value === "string" &&
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value) &&
    Number.isFinite(Date.parse(value)) &&
    new Date(value).toISOString() === value
  );
}

function isNullableBoundedString(value: unknown, maximum: number): value is string | null {
  return value === null || isBoundedString(value, 0, maximum);
}

function isBoundedString(value: unknown, minimum: number, maximum: number): value is string {
  if (typeof value !== "string") return false;
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

function isSafeIntegerBetween(value: unknown, minimum: number, maximum: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= minimum && value <= maximum;
}

function valid<T>(value: T): ParseResult<T> {
  return {ok: true, value};
}

function invalid<T>(code: string, message: string): ParseResult<T> {
  return {ok: false, error: {code, message}};
}

function unknownField<T>(): ParseResult<T> {
  return invalid("UNKNOWN_FIELD", "Contract contains an unknown field");
}
