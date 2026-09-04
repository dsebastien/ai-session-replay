import type {FontSpec, ThemeSpec} from "./types";

export const INDEXED_LIBRARY_SCHEMA_VERSION = 1 as const;
export const MAX_INDEXED_PAGE_SIZE = 200;
export const MAX_SELECTION_CHANGE_COUNT = 500;
export const MAX_PRESENTATION_ENTRY_COUNT = 100_000;
export const DEFAULT_PRESENTATION_DELAY_MS = 5_000;
export const MIN_PRESENTATION_DELAY_MS = 250;
export const MAX_PRESENTATION_DELAY_MS = 10_000;
export const MIN_PRESENTATION_SPEED = 0.25;
export const MAX_PRESENTATION_SPEED = 4;

const MAX_OPAQUE_ID_LENGTH = 256;
const MAX_QUERY_LENGTH = 256;
const MAX_TITLE_LENGTH = 512;
const MAX_ENTRY_TEXT_LENGTH = 2_000_000;
const MAX_TOOL_NAME_LENGTH = 256;
const MAX_DISPLAY_PATH_LENGTH = 32_768;
const MAX_SOURCE_TYPE_LENGTH = 256;
const MAX_DIAGNOSTIC_COUNT = 1_000;
const MAX_SOURCE_DURATION_MS = 7 * 24 * 60 * 60 * 1_000;
export const MAX_PRESENTATION_DURATION_MS = 2 * 60 * 60 * 1_000;
const MAX_DELETION_ARTIFACT_COUNT = 64;

export interface LibraryContractValidationError {
  readonly code: string;
  readonly message: string;
}

export type LibraryContractValidationResult<T> =
  | Readonly<{ok: true; value: T}>
  | Readonly<{ok: false; error: LibraryContractValidationError}>;

export type ContentAvailability = "available" | "partial" | "unavailable";
export type IndexedSessionSourceV1 = "claude-code" | "codex" | "copilot-cli" | "vscode-copilot";
export type IndexedSessionSortOrderV1 = "newest" | "oldest";

export interface ContentAvailabilityV1 {
  readonly reasoning: "available" | "unavailable";
  readonly toolDetails: ContentAvailability;
}

interface EntryBaseV1 {
  readonly entryKey: string;
  readonly ordinal: number;
  readonly atMs: number;
}

export type ToolDetailV1 =
  | Readonly<{availability: "unavailable"}>
  | Readonly<{
      availability: "available";
      arguments: string | null;
      result: string | null;
    }>;

export type SessionEntryV1 =
  | Readonly<EntryBaseV1 & {kind: "user"; text: string}>
  | Readonly<EntryBaseV1 & {kind: "assistant"; markdown: string}>
  | Readonly<EntryBaseV1 & {kind: "reasoning"; text: string}>
  | Readonly<
      EntryBaseV1 & {
        kind: "tool-call";
        name: string;
        status: "pending" | "running" | "succeeded" | "failed";
        summary: string;
        detail: ToolDetailV1;
      }
    >
  | Readonly<
      EntryBaseV1 & {
        kind: "file-change";
        displayPath: string;
        summary: string;
      }
    >
  | Readonly<EntryBaseV1 & {kind: "unknown"; sourceType: string}>;

export interface SelectableEntryV1 {
  readonly entry: SessionEntryV1;
  readonly selected: boolean;
}

export interface IndexedSessionSummaryV1 {
  readonly schemaVersion: 1;
  readonly sessionId: string;
  readonly source: IndexedSessionSourceV1;
  readonly title: string;
  readonly createdAtMs: number | null;
  readonly lastIndexedAtMs: number;
  readonly sourcePresent: boolean;
  readonly entryCount: number;
  readonly selectedEntryCount: number;
  readonly durationMs: number;
  readonly diagnosticCount: number;
  readonly contentAvailability: ContentAvailabilityV1;
}

export interface IndexedSessionListRequestV1 {
  readonly schemaVersion: 1;
  readonly source: IndexedSessionSourceV1 | null;
  readonly query: string;
  readonly sortOrder: IndexedSessionSortOrderV1;
  readonly cursor: string | null;
  readonly pageSize: number;
}

export interface IndexedSessionPageV1 {
  readonly schemaVersion: 1;
  readonly sortOrder: IndexedSessionSortOrderV1;
  readonly items: readonly IndexedSessionSummaryV1[];
  readonly nextCursor: string | null;
}

export interface SessionRevisionV1 {
  readonly schemaVersion: 1;
  readonly revisionId: string;
  readonly sessionId: string;
  readonly indexedAtMs: number;
  readonly entryCount: number;
  readonly durationMs: number;
  readonly diagnosticCount: number;
}

export interface IndexedEntryPageV1 {
  readonly schemaVersion: 1;
  readonly sessionId: string;
  readonly revisionId: string;
  readonly entries: readonly SelectableEntryV1[];
  readonly nextCursor: string | null;
  readonly totalEntryCount: number;
}

export interface VisibilityPreferencesV1 {
  readonly showToolCalls: boolean;
  readonly showToolDetails: boolean;
  readonly showReasoning: boolean;
}

export interface PresentationTimingV1 {
  readonly entryDelayMs: number;
  readonly playbackSpeed: number;
}

export interface AppearancePreferencesV1 {
  readonly theme: ThemeSpec;
  readonly font: FontSpec;
}

export interface SessionPreferencesV1 {
  readonly schemaVersion: 1;
  readonly visibility: VisibilityPreferencesV1;
  readonly timing: PresentationTimingV1;
  readonly appearance: AppearancePreferencesV1;
}

export interface IndexedSessionDetailV1 {
  readonly schemaVersion: 1;
  readonly summary: IndexedSessionSummaryV1;
  readonly revision: SessionRevisionV1;
  readonly entryPage: IndexedEntryPageV1;
  readonly preferences: SessionPreferencesV1;
}

export interface EntrySelectionChangeV1 {
  readonly entryKey: string;
  readonly selected: boolean;
}

export interface SetEntrySelectionsRequestV1 {
  readonly schemaVersion: 1;
  readonly sessionId: string;
  readonly revisionId: string;
  readonly changes: readonly EntrySelectionChangeV1[];
}

export interface RenameIndexedSessionRequestV1 {
  readonly schemaVersion: 1;
  readonly sessionId: string;
  readonly title: string;
}

export interface SetSessionPreferencesRequestV1 {
  readonly schemaVersion: 1;
  readonly sessionId: string;
  readonly revisionId: string;
  readonly preferences: SessionPreferencesV1;
}

interface RefreshCountsV1 {
  readonly discoveredCount: number;
  readonly processedCount: number;
  readonly indexedCount: number;
  readonly unchangedCount: number;
  readonly failedCount: number;
  readonly skippedCount: number;
  readonly warningCount: number;
}

export type IndexRefreshStateV1 =
  | Readonly<{
      schemaVersion: 1;
      status: "idle";
      lastCompletedAtMs: number | null;
    }>
  | Readonly<
      RefreshCountsV1 & {
        schemaVersion: 1;
        status: "running";
        generation: number;
        startedAtMs: number;
      }
    >
  | Readonly<
      RefreshCountsV1 & {
        schemaVersion: 1;
        status: "completed";
        generation: number;
        startedAtMs: number;
        completedAtMs: number;
      }
    >
  | Readonly<
      RefreshCountsV1 & {
        schemaVersion: 1;
        status: "failed";
        generation: number;
        startedAtMs: number;
        completedAtMs: number;
        errorCode: string;
      }
    >;

export interface DeleteIndexedSessionRequestV1 {
  readonly schemaVersion: 1;
  readonly mode: "library-only";
  readonly sessionId: string;
}

export interface PrepareSourceDeletionRequestV1 {
  readonly schemaVersion: 1;
  readonly mode: "prepare-source-deletion";
  readonly sessionId: string;
}

export interface SourceDeletionConfirmationV1 {
  readonly schemaVersion: 1;
  readonly sessionId: string;
  readonly confirmationToken: string;
  readonly artifactCount: number;
  readonly expiresAtMs: number;
}

export interface DeleteIndexedSessionWithSourceRequestV1 {
  readonly schemaVersion: 1;
  readonly mode: "library-and-source";
  readonly sessionId: string;
  readonly confirmationToken: string;
}

export interface RestoreSuppressedSourceRequestV1 {
  readonly schemaVersion: 1;
  readonly suppressionId: string;
}

export interface SuppressedSourceV1 {
  readonly schemaVersion: 1;
  readonly suppressionId: string;
  readonly source: IndexedSessionSourceV1;
  readonly sourceDeleted: boolean;
  readonly suppressedAtMs: number;
}

export interface SuppressedSourceListRequestV1 {
  readonly schemaVersion: 1;
  readonly cursor: string | null;
  readonly pageSize: number;
}

export interface SuppressedSourcePageV1 {
  readonly schemaVersion: 1;
  readonly items: readonly SuppressedSourceV1[];
  readonly nextCursor: string | null;
}

export interface RestoreSuppressedSourceResultV1 {
  readonly schemaVersion: 1;
  readonly suppressionId: string;
}

export interface ResetLocalDatabaseRequestV1 {
  readonly schemaVersion: 1;
  readonly mode: "reset-local-database";
}

export interface ResetLocalDatabaseResultV1 {
  readonly schemaVersion: 1;
  readonly resetAtMs: number;
}

export type PresentedToolDetailV1 = Readonly<{
  arguments: string | null;
  result: string | null;
}>;

export type PresentationEntryContentV1 =
  | Readonly<EntryBaseV1 & {kind: "user"; text: string}>
  | Readonly<EntryBaseV1 & {kind: "assistant"; markdown: string}>
  | Readonly<EntryBaseV1 & {kind: "reasoning"; text: string}>
  | Readonly<
      EntryBaseV1 & {
        kind: "tool-call";
        name: string;
        status: "pending" | "running" | "succeeded" | "failed";
        summary: string;
        detail: PresentedToolDetailV1 | null;
      }
    >
  | Readonly<
      EntryBaseV1 & {
        kind: "file-change";
        displayPath: string;
        summary: string;
      }
    >
  | Readonly<EntryBaseV1 & {kind: "unknown"; sourceType: string}>;

export interface PresentationPlanEntryV1 {
  readonly entry: PresentationEntryContentV1;
  readonly revealAtMs: number;
}

export interface PresentationPlanV1 {
  readonly schemaVersion: 1;
  readonly planId: string;
  readonly sessionId: string;
  readonly revisionId: string;
  readonly sessionTitle: string;
  readonly createdAtMs: number;
  readonly entries: readonly PresentationPlanEntryV1[];
  readonly preferences: SessionPreferencesV1;
  readonly durationMs: number;
  readonly fps: 30;
  readonly width: 1920;
  readonly height: 1080;
}

type ParseResult<T> = LibraryContractValidationResult<T>;
type JsonRecord = Record<string, unknown>;

export function validateIndexedSessionListRequest(
  input: unknown,
): ParseResult<IndexedSessionListRequestV1> {
  const record = parseRecord(
    input,
    ["schemaVersion", "source", "query", "sortOrder", "cursor", "pageSize"],
    "INVALID_LIST_REQUEST",
  );
  if (!record.ok) return record;
  const {schemaVersion, source, query, sortOrder, cursor, pageSize} = record.value;
  if (schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  if (source !== null && !isSessionSource(source)) return invalid("INVALID_SOURCE", "Session source is invalid");
  if (!isBoundedString(query, 0, MAX_QUERY_LENGTH)) return invalid("INVALID_QUERY", "Search query is invalid");
  if (sortOrder !== "newest" && sortOrder !== "oldest") return invalid("INVALID_SORT_ORDER", "Sort order is invalid");
  if (cursor !== null && !isListCursor(cursor, sortOrder)) return invalid("INVALID_CURSOR", "Page cursor is invalid");
  if (!isIntegerBetween(pageSize, 1, MAX_INDEXED_PAGE_SIZE)) return invalid("INVALID_PAGE_SIZE", "Page size is invalid");
  return valid({schemaVersion: 1, source, query, sortOrder, cursor, pageSize});
}

export function validateIndexedSessionPage(input: unknown): ParseResult<IndexedSessionPageV1> {
  const record = parseRecord(input, ["schemaVersion", "sortOrder", "items", "nextCursor"], "INVALID_SESSION_PAGE");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  if (!Array.isArray(record.value.items)) return invalid("INVALID_SESSION_PAGE", "Session page is invalid");
  const sortOrder = record.value.sortOrder;
  if (sortOrder !== "newest" && sortOrder !== "oldest") return invalid("INVALID_SORT_ORDER", "Sort order is invalid");
  if (record.value.items.length > MAX_INDEXED_PAGE_SIZE) return invalid("PAGE_LIMIT_EXCEEDED", "Session page exceeds the supported limit");
  const items: IndexedSessionSummaryV1[] = [];
  const ids = new Set<string>();
  for (const candidate of record.value.items) {
    const summary = parseSessionSummary(candidate);
    if (!summary.ok) return summary;
    if (ids.has(summary.value.sessionId)) return invalid("DUPLICATE_SESSION_ID", "Session page contains duplicate IDs");
    ids.add(summary.value.sessionId);
    items.push(summary.value);
  }
  const nextCursor = record.value.nextCursor;
  if (nextCursor !== null && !isListCursor(nextCursor, sortOrder)) return invalid("INVALID_CURSOR", "Page cursor is invalid");
  return valid({schemaVersion: 1, sortOrder, items, nextCursor});
}

export function validateIndexedSessionDetail(input: unknown): ParseResult<IndexedSessionDetailV1> {
  const record = parseRecord(input, ["schemaVersion", "summary", "revision", "entryPage", "preferences"], "INVALID_SESSION_DETAIL");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const summary = parseSessionSummary(record.value.summary);
  if (!summary.ok) return summary;
  const revision = parseRevision(record.value.revision);
  if (!revision.ok) return revision;
  const entryPage = parseEntryPage(record.value.entryPage);
  if (!entryPage.ok) return entryPage;
  const preferences = parsePreferences(record.value.preferences);
  if (!preferences.ok) return preferences;
  if (
    summary.value.sessionId !== revision.value.sessionId ||
    summary.value.sessionId !== entryPage.value.sessionId
  ) {
    return invalid("SESSION_MISMATCH", "Session identities do not match");
  }
  if (revision.value.revisionId !== entryPage.value.revisionId) {
    return invalid("REVISION_MISMATCH", "Revision identities do not match");
  }
  if (
    summary.value.entryCount !== revision.value.entryCount ||
    summary.value.entryCount !== entryPage.value.totalEntryCount ||
    summary.value.durationMs !== revision.value.durationMs ||
    summary.value.diagnosticCount !== revision.value.diagnosticCount
  ) {
    return invalid("SESSION_METADATA_MISMATCH", "Session metadata does not match the current revision");
  }
  const entries = entryPage.value.entries.map(({entry}) => entry);
  if (
    summary.value.contentAvailability.reasoning === "unavailable" &&
    entries.some(({kind}) => kind === "reasoning")
  ) {
    return invalid("CONTENT_AVAILABILITY_MISMATCH", "Reasoning availability contradicts the entries");
  }
  const toolDetails = entries
    .filter((entry): entry is Extract<SessionEntryV1, {kind: "tool-call"}> => entry.kind === "tool-call")
    .map(({detail}) => detail.availability);
  if (
    (summary.value.contentAvailability.toolDetails === "unavailable" && toolDetails.includes("available")) ||
    (summary.value.contentAvailability.toolDetails === "available" && toolDetails.includes("unavailable"))
  ) {
    return invalid("CONTENT_AVAILABILITY_MISMATCH", "Tool-detail availability contradicts the entries");
  }
  return valid({schemaVersion: 1, summary: summary.value, revision: revision.value, entryPage: entryPage.value, preferences: preferences.value});
}

export function validateSetEntrySelectionsRequest(input: unknown): ParseResult<SetEntrySelectionsRequestV1> {
  const record = parseRecord(input, ["schemaVersion", "sessionId", "revisionId", "changes"], "INVALID_SELECTION_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  const revisionId = parseOpaqueId(record.value.revisionId);
  if (!revisionId.ok) return revisionId;
  if (!Array.isArray(record.value.changes)) return invalid("INVALID_SELECTION_REQUEST", "Selection changes are invalid");
  if (record.value.changes.length > MAX_SELECTION_CHANGE_COUNT) return invalid("SELECTION_CHANGE_LIMIT_EXCEEDED", "Selection change batch exceeds the supported limit");
  const changes: EntrySelectionChangeV1[] = [];
  const keys = new Set<string>();
  for (const candidate of record.value.changes) {
    const parsed = parseSelectionChange(candidate);
    if (!parsed.ok) return parsed;
    if (keys.has(parsed.value.entryKey)) return invalid("DUPLICATE_ENTRY_KEY", "Selection changes contain duplicate entry keys");
    keys.add(parsed.value.entryKey);
    changes.push(parsed.value);
  }
  return valid({schemaVersion: 1, sessionId: sessionId.value, revisionId: revisionId.value, changes});
}

export function validateRenameIndexedSessionRequest(input: unknown): ParseResult<RenameIndexedSessionRequestV1> {
  const record = parseRecord(input, ["schemaVersion", "sessionId", "title"], "INVALID_RENAME_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  if (
    !isBoundedString(record.value.title, 1, MAX_TITLE_LENGTH) ||
    record.value.title.trim() !== record.value.title
  ) {
    return invalid("INVALID_SESSION_TITLE", "Session title is invalid");
  }
  return valid({schemaVersion: 1, sessionId: sessionId.value, title: record.value.title});
}

export function validateSetSessionPreferencesRequest(input: unknown): ParseResult<SetSessionPreferencesRequestV1> {
  const record = parseRecord(input, ["schemaVersion", "sessionId", "revisionId", "preferences"], "INVALID_PREFERENCES_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  const revisionId = parseOpaqueId(record.value.revisionId);
  if (!revisionId.ok) return revisionId;
  const preferences = parsePreferences(record.value.preferences);
  if (!preferences.ok) return preferences;
  return valid({schemaVersion: 1, sessionId: sessionId.value, revisionId: revisionId.value, preferences: preferences.value});
}

export function validateIndexRefreshState(input: unknown): ParseResult<IndexRefreshStateV1> {
  if (!isRecord(input)) return invalid("INVALID_REFRESH_STATE", "Refresh state is invalid");
  if (input.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  if (input.status === "idle") {
    if (!hasOnlyKeys(input, ["schemaVersion", "status", "lastCompletedAtMs"])) return invalid("UNKNOWN_FIELD", "Contract contains an unknown field");
    if (input.lastCompletedAtMs !== null && !isTimestamp(input.lastCompletedAtMs)) return invalid("INVALID_TIMESTAMP", "Refresh timestamp is invalid");
    return valid({schemaVersion: 1, status: "idle", lastCompletedAtMs: input.lastCompletedAtMs});
  }
  const terminal = input.status === "completed" || input.status === "failed";
  const keys = terminal
    ? ["schemaVersion", "status", "generation", "startedAtMs", "completedAtMs", "discoveredCount", "processedCount", "indexedCount", "unchangedCount", "failedCount", "skippedCount", "warningCount", ...(input.status === "failed" ? ["errorCode"] : [])]
    : ["schemaVersion", "status", "generation", "startedAtMs", "discoveredCount", "processedCount", "indexedCount", "unchangedCount", "failedCount", "skippedCount", "warningCount"];
  if (input.status !== "running" && !terminal) return invalid("INVALID_REFRESH_STATUS", "Refresh status is invalid");
  if (!hasOnlyKeys(input, keys)) return invalid("UNKNOWN_FIELD", "Contract contains an unknown field");
  if (!isSafeCount(input.generation) || input.generation === 0 || !isTimestamp(input.startedAtMs)) return invalid("INVALID_REFRESH_STATE", "Refresh state is invalid");
  const counts = parseRefreshCounts(input);
  if (!counts.ok) return counts;
  if (!terminal) return valid({schemaVersion: 1, status: "running", generation: input.generation, startedAtMs: input.startedAtMs, ...counts.value});
  if (!isTimestamp(input.completedAtMs) || input.completedAtMs < input.startedAtMs) return invalid("INVALID_TIMESTAMP", "Refresh timestamp is invalid");
  if (input.status === "completed") return valid({schemaVersion: 1, status: "completed", generation: input.generation, startedAtMs: input.startedAtMs, completedAtMs: input.completedAtMs, ...counts.value});
  if (!isSafeCode(input.errorCode)) return invalid("INVALID_ERROR_CODE", "Refresh error code is invalid");
  return valid({schemaVersion: 1, status: "failed", generation: input.generation, startedAtMs: input.startedAtMs, completedAtMs: input.completedAtMs, errorCode: input.errorCode, ...counts.value});
}

export function validateDeleteIndexedSessionRequest(input: unknown): ParseResult<DeleteIndexedSessionRequestV1> {
  if (!isRecord(input)) return invalid("INVALID_DELETION_REQUEST", "Deletion request is invalid");
  if (input.mode !== "library-only") return invalid("INVALID_DELETION_MODE", "Deletion mode is invalid");
  const record = parseRecord(input, ["schemaVersion", "mode", "sessionId"], "INVALID_DELETION_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  return valid({schemaVersion: 1, mode: "library-only", sessionId: sessionId.value});
}

export function validatePrepareSourceDeletionRequest(input: unknown): ParseResult<PrepareSourceDeletionRequestV1> {
  if (!isRecord(input)) return invalid("INVALID_DELETION_REQUEST", "Deletion request is invalid");
  if (input.mode !== "prepare-source-deletion") return invalid("INVALID_DELETION_MODE", "Deletion mode is invalid");
  const record = parseRecord(input, ["schemaVersion", "mode", "sessionId"], "INVALID_DELETION_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  return valid({schemaVersion: 1, mode: "prepare-source-deletion", sessionId: sessionId.value});
}

export function validateSourceDeletionConfirmation(input: unknown): ParseResult<SourceDeletionConfirmationV1> {
  const record = parseRecord(input, ["schemaVersion", "sessionId", "confirmationToken", "artifactCount", "expiresAtMs"], "INVALID_DELETION_CONFIRMATION");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  const confirmationToken = parseConfirmationToken(record.value.confirmationToken);
  if (!confirmationToken.ok) return confirmationToken;
  if (!isIntegerBetween(record.value.artifactCount, 1, MAX_DELETION_ARTIFACT_COUNT)) return invalid("INVALID_ARTIFACT_COUNT", "Deletion artifact count is invalid");
  if (!isTimestamp(record.value.expiresAtMs)) return invalid("INVALID_TIMESTAMP", "Confirmation expiry is invalid");
  return valid({schemaVersion: 1, sessionId: sessionId.value, confirmationToken: confirmationToken.value, artifactCount: record.value.artifactCount, expiresAtMs: record.value.expiresAtMs});
}

export function validateDeleteIndexedSessionWithSourceRequest(input: unknown): ParseResult<DeleteIndexedSessionWithSourceRequestV1> {
  if (!isRecord(input)) return invalid("INVALID_DELETION_REQUEST", "Deletion request is invalid");
  if (input.mode !== "library-and-source") return invalid("INVALID_DELETION_MODE", "Deletion mode is invalid");
  const record = parseRecord(input, ["schemaVersion", "mode", "sessionId", "confirmationToken"], "INVALID_DELETION_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  const confirmationToken = parseConfirmationToken(record.value.confirmationToken);
  if (!confirmationToken.ok) return confirmationToken;
  return valid({schemaVersion: 1, mode: "library-and-source", sessionId: sessionId.value, confirmationToken: confirmationToken.value});
}

export function validateRestoreSuppressedSourceRequest(input: unknown): ParseResult<RestoreSuppressedSourceRequestV1> {
  const record = parseRecord(input, ["schemaVersion", "suppressionId"], "INVALID_RESTORE_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const suppressionId = parseOpaqueId(record.value.suppressionId);
  if (!suppressionId.ok) return suppressionId;
  return valid({schemaVersion: 1, suppressionId: suppressionId.value});
}

export function validateSuppressedSource(input: unknown): ParseResult<SuppressedSourceV1> {
  const record = parseRecord(input, ["schemaVersion", "suppressionId", "source", "sourceDeleted", "suppressedAtMs"], "INVALID_SUPPRESSION");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const suppressionId = parseOpaqueId(record.value.suppressionId);
  if (!suppressionId.ok) return suppressionId;
  if (!isSessionSource(record.value.source) || typeof record.value.sourceDeleted !== "boolean" || !isTimestamp(record.value.suppressedAtMs)) {
    return invalid("INVALID_SUPPRESSION", "Suppressed source is invalid");
  }
  return valid({
    schemaVersion: 1,
    suppressionId: suppressionId.value,
    source: record.value.source,
    sourceDeleted: record.value.sourceDeleted,
    suppressedAtMs: record.value.suppressedAtMs,
  });
}

export function validateSuppressedSourceListRequest(input: unknown): ParseResult<SuppressedSourceListRequestV1> {
  const record = parseRecord(input, ["schemaVersion", "cursor", "pageSize"], "INVALID_SUPPRESSION_LIST_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  if (record.value.cursor !== null && !isOpaqueId(record.value.cursor)) return invalid("INVALID_OPAQUE_ID", "Opaque identifier is invalid");
  if (!isIntegerBetween(record.value.pageSize, 1, MAX_INDEXED_PAGE_SIZE)) return invalid("INVALID_PAGE_SIZE", "Page size is invalid");
  return valid({schemaVersion: 1, cursor: record.value.cursor, pageSize: record.value.pageSize});
}

export function validateSuppressedSourcePage(input: unknown): ParseResult<SuppressedSourcePageV1> {
  const record = parseRecord(input, ["schemaVersion", "items", "nextCursor"], "INVALID_SUPPRESSION_PAGE");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  if (!Array.isArray(record.value.items)) return invalid("INVALID_SUPPRESSION_PAGE", "Suppressed-source page is invalid");
  if (record.value.items.length > MAX_INDEXED_PAGE_SIZE) return invalid("PAGE_LIMIT_EXCEEDED", "Suppressed-source page exceeds the supported limit");
  const items: SuppressedSourceV1[] = [];
  const ids = new Set<string>();
  for (const candidate of record.value.items) {
    const parsed = validateSuppressedSource(candidate);
    if (!parsed.ok) return parsed;
    if (ids.has(parsed.value.suppressionId)) return invalid("DUPLICATE_SUPPRESSION_ID", "Suppressed-source page contains duplicate IDs");
    ids.add(parsed.value.suppressionId);
    items.push(parsed.value);
  }
  const nextCursor = record.value.nextCursor;
  if (nextCursor !== null && !isOpaqueId(nextCursor)) return invalid("INVALID_OPAQUE_ID", "Opaque identifier is invalid");
  return valid({schemaVersion: 1, items, nextCursor});
}

export function validateRestoreSuppressedSourceResult(input: unknown): ParseResult<RestoreSuppressedSourceResultV1> {
  const record = parseRecord(input, ["schemaVersion", "suppressionId"], "INVALID_RESTORE_RESULT");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const suppressionId = parseOpaqueId(record.value.suppressionId);
  if (!suppressionId.ok) return suppressionId;
  return valid({schemaVersion: 1, suppressionId: suppressionId.value});
}

export function validateResetLocalDatabaseRequest(input: unknown): ParseResult<ResetLocalDatabaseRequestV1> {
  const record = parseRecord(input, ["schemaVersion", "mode"], "INVALID_RESET_REQUEST");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  if (record.value.mode !== "reset-local-database") return invalid("INVALID_RESET_MODE", "Reset mode is invalid");
  return valid({schemaVersion: 1, mode: "reset-local-database"});
}

export function validateResetLocalDatabaseResult(input: unknown): ParseResult<ResetLocalDatabaseResultV1> {
  const record = parseRecord(input, ["schemaVersion", "resetAtMs"], "INVALID_RESET_RESULT");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  if (!isTimestamp(record.value.resetAtMs)) return invalid("INVALID_TIMESTAMP", "Reset timestamp is invalid");
  return valid({schemaVersion: 1, resetAtMs: record.value.resetAtMs});
}

export function validatePresentationPlan(input: unknown): ParseResult<PresentationPlanV1> {
  const record = parseRecord(input, ["schemaVersion", "planId", "sessionId", "revisionId", "sessionTitle", "createdAtMs", "entries", "preferences", "durationMs", "fps", "width", "height"], "INVALID_PRESENTATION_PLAN");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const planId = parseOpaqueId(record.value.planId);
  if (!planId.ok) return planId;
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  const revisionId = parseOpaqueId(record.value.revisionId);
  if (!revisionId.ok) return revisionId;
  if (!isBoundedString(record.value.sessionTitle, 1, MAX_TITLE_LENGTH) || !isTimestamp(record.value.createdAtMs)) return invalid("INVALID_PRESENTATION_PLAN", "Presentation metadata is invalid");
  if (!Array.isArray(record.value.entries)) return invalid("INVALID_PRESENTATION_PLAN", "Presentation entries are invalid");
  if (record.value.entries.length === 0) return invalid("EMPTY_PRESENTATION_PLAN", "Presentation plan has no entries");
  if (record.value.entries.length > MAX_PRESENTATION_ENTRY_COUNT) return invalid("PRESENTATION_ENTRY_LIMIT_EXCEEDED", "Presentation plan exceeds the supported entry limit");
  const entries: PresentationPlanEntryV1[] = [];
  const entryKeys = new Set<string>();
  let previousOrdinal = -1;
  let previousAtMs = -1;
  let previousRevealAtMs = -1;
  for (const candidate of record.value.entries) {
    const parsed = parsePresentationPlanEntry(candidate);
    if (!parsed.ok) return parsed;
    if (entryKeys.has(parsed.value.entry.entryKey) || parsed.value.entry.ordinal <= previousOrdinal || parsed.value.entry.atMs < previousAtMs || parsed.value.revealAtMs < previousRevealAtMs) return invalid("INVALID_PRESENTATION_ORDER", "Presentation entries are not in deterministic order");
    entryKeys.add(parsed.value.entry.entryKey);
    previousOrdinal = parsed.value.entry.ordinal;
    previousAtMs = parsed.value.entry.atMs;
    previousRevealAtMs = parsed.value.revealAtMs;
    entries.push(parsed.value);
  }
  const preferences = parsePreferences(record.value.preferences);
  if (!preferences.ok) return preferences;
  if (
    (!preferences.value.visibility.showReasoning && entries.some(({entry}) => entry.kind === "reasoning")) ||
    (!preferences.value.visibility.showToolCalls && entries.some(({entry}) => entry.kind === "tool-call")) ||
    (!preferences.value.visibility.showToolDetails && entries.some(({entry}) => entry.kind === "tool-call" && entry.detail !== null))
  ) {
    return invalid("PRESENTATION_VISIBILITY_MISMATCH", "Presentation entries contradict visibility preferences");
  }
  if (!isIntegerBetween(record.value.durationMs, 1, MAX_PRESENTATION_DURATION_MS) || previousRevealAtMs >= record.value.durationMs) return invalid("INVALID_PRESENTATION_DURATION", "Presentation duration is invalid");
  if (record.value.fps !== 30 || record.value.width !== 1920 || record.value.height !== 1080) return invalid("INVALID_RENDER_SETTINGS", "Presentation render settings are invalid");
  return valid({schemaVersion: 1, planId: planId.value, sessionId: sessionId.value, revisionId: revisionId.value, sessionTitle: record.value.sessionTitle, createdAtMs: record.value.createdAtMs, entries, preferences: preferences.value, durationMs: record.value.durationMs, fps: 30, width: 1920, height: 1080});
}

function parseSessionSummary(input: unknown): ParseResult<IndexedSessionSummaryV1> {
  const record = parseRecord(input, ["schemaVersion", "sessionId", "source", "title", "createdAtMs", "lastIndexedAtMs", "sourcePresent", "entryCount", "selectedEntryCount", "durationMs", "diagnosticCount", "contentAvailability"], "INVALID_SESSION_SUMMARY");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  if (!isSessionSource(record.value.source)) return invalid("INVALID_SOURCE", "Session source is invalid");
  if (!isBoundedString(record.value.title, 1, MAX_TITLE_LENGTH)) return invalid("INVALID_TITLE", "Session title is invalid");
  if (record.value.createdAtMs !== null && !isTimestamp(record.value.createdAtMs)) return invalid("INVALID_TIMESTAMP", "Creation timestamp is invalid");
  if (!isTimestamp(record.value.lastIndexedAtMs)) return invalid("INVALID_TIMESTAMP", "Index timestamp is invalid");
  if (typeof record.value.sourcePresent !== "boolean") return invalid("INVALID_SOURCE_PRESENCE", "Source presence is invalid");
  if (!isIntegerBetween(record.value.entryCount, 1, MAX_PRESENTATION_ENTRY_COUNT) || !isIntegerBetween(record.value.selectedEntryCount, 0, record.value.entryCount)) return invalid("INVALID_ENTRY_COUNT", "Session entry counts are invalid");
  if (!isIntegerBetween(record.value.durationMs, 0, MAX_SOURCE_DURATION_MS)) return invalid("INVALID_SESSION_DURATION", "Session duration is invalid");
  if (!isIntegerBetween(record.value.diagnosticCount, 0, MAX_DIAGNOSTIC_COUNT)) return invalid("INVALID_DIAGNOSTIC_COUNT", "Diagnostic count is invalid");
  const contentAvailability = parseContentAvailability(record.value.contentAvailability);
  if (!contentAvailability.ok) return contentAvailability;
  return valid({schemaVersion: 1, sessionId: sessionId.value, source: record.value.source, title: record.value.title, createdAtMs: record.value.createdAtMs, lastIndexedAtMs: record.value.lastIndexedAtMs, sourcePresent: record.value.sourcePresent, entryCount: record.value.entryCount, selectedEntryCount: record.value.selectedEntryCount, durationMs: record.value.durationMs, diagnosticCount: record.value.diagnosticCount, contentAvailability: contentAvailability.value});
}

function parseRevision(input: unknown): ParseResult<SessionRevisionV1> {
  const record = parseRecord(input, ["schemaVersion", "revisionId", "sessionId", "indexedAtMs", "entryCount", "durationMs", "diagnosticCount"], "INVALID_REVISION");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const revisionId = parseOpaqueId(record.value.revisionId);
  if (!revisionId.ok) return revisionId;
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  if (!isTimestamp(record.value.indexedAtMs) || !isIntegerBetween(record.value.entryCount, 1, MAX_PRESENTATION_ENTRY_COUNT) || !isIntegerBetween(record.value.durationMs, 0, MAX_SOURCE_DURATION_MS) || !isIntegerBetween(record.value.diagnosticCount, 0, MAX_DIAGNOSTIC_COUNT)) return invalid("INVALID_REVISION", "Revision metadata is invalid");
  return valid({schemaVersion: 1, revisionId: revisionId.value, sessionId: sessionId.value, indexedAtMs: record.value.indexedAtMs, entryCount: record.value.entryCount, durationMs: record.value.durationMs, diagnosticCount: record.value.diagnosticCount});
}

function parseEntryPage(input: unknown): ParseResult<IndexedEntryPageV1> {
  const record = parseRecord(input, ["schemaVersion", "sessionId", "revisionId", "entries", "nextCursor", "totalEntryCount"], "INVALID_ENTRY_PAGE");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const sessionId = parseOpaqueId(record.value.sessionId);
  if (!sessionId.ok) return sessionId;
  const revisionId = parseOpaqueId(record.value.revisionId);
  if (!revisionId.ok) return revisionId;
  if (!Array.isArray(record.value.entries) || record.value.entries.length > MAX_INDEXED_PAGE_SIZE) return invalid("PAGE_LIMIT_EXCEEDED", "Entry page exceeds the supported limit");
  if (!isIntegerBetween(record.value.totalEntryCount, 1, MAX_PRESENTATION_ENTRY_COUNT) || record.value.entries.length > record.value.totalEntryCount) return invalid("INVALID_ENTRY_COUNT", "Entry count is invalid");
  const entries: SelectableEntryV1[] = [];
  const keys = new Set<string>();
  let previousOrdinal = -1;
  let previousAtMs = -1;
  for (const candidate of record.value.entries) {
    const parsed = parseSelectableEntry(candidate);
    if (!parsed.ok) return parsed;
    if (keys.has(parsed.value.entry.entryKey) || parsed.value.entry.ordinal <= previousOrdinal || parsed.value.entry.atMs < previousAtMs) return invalid("INVALID_ENTRY_ORDER", "Entries are not in deterministic order");
    keys.add(parsed.value.entry.entryKey);
    previousOrdinal = parsed.value.entry.ordinal;
    previousAtMs = parsed.value.entry.atMs;
    entries.push(parsed.value);
  }
  const nextCursor = record.value.nextCursor;
  if (nextCursor !== null && !isOpaqueId(nextCursor)) return invalid("INVALID_CURSOR", "Page cursor is invalid");
  return valid({schemaVersion: 1, sessionId: sessionId.value, revisionId: revisionId.value, entries, nextCursor, totalEntryCount: record.value.totalEntryCount});
}

function parseSelectableEntry(input: unknown): ParseResult<SelectableEntryV1> {
  const record = parseRecord(input, ["entry", "selected"], "INVALID_SELECTABLE_ENTRY");
  if (!record.ok) return record;
  const entry = parseSessionEntry(record.value.entry);
  if (!entry.ok) return entry;
  if (typeof record.value.selected !== "boolean") return invalid("INVALID_SELECTION", "Entry selection is invalid");
  return valid({entry: entry.value, selected: record.value.selected});
}

function parseSessionEntry(input: unknown): ParseResult<SessionEntryV1> {
  if (!isRecord(input)) return invalid("INVALID_ENTRY", "Session entry is invalid");
  const base = parseEntryBase(input);
  if (!base.ok) return base;
  switch (input.kind) {
    case "user":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "text"])) return unknownField();
      return isBoundedString(input.text, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "user", text: input.text}) : invalid("INVALID_ENTRY", "User entry is invalid");
    case "assistant":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "markdown"])) return unknownField();
      return isBoundedString(input.markdown, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "assistant", markdown: input.markdown}) : invalid("INVALID_ENTRY", "Assistant entry is invalid");
    case "reasoning":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "text"])) return unknownField();
      return isBoundedString(input.text, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "reasoning", text: input.text}) : invalid("INVALID_ENTRY", "Reasoning entry is invalid");
    case "tool-call": {
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "name", "status", "summary", "detail"])) return unknownField();
      const detail = parseToolDetail(input.detail);
      if (!detail.ok) return detail;
      if (!isBoundedString(input.name, 1, MAX_TOOL_NAME_LENGTH) || !isToolStatus(input.status) || !isBoundedString(input.summary, 0, MAX_ENTRY_TEXT_LENGTH)) return invalid("INVALID_ENTRY", "Tool entry is invalid");
      return valid({...base.value, kind: "tool-call", name: input.name, status: input.status, summary: input.summary, detail: detail.value});
    }
    case "file-change":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "displayPath", "summary"])) return unknownField();
      return isBoundedString(input.displayPath, 1, MAX_DISPLAY_PATH_LENGTH) && isBoundedString(input.summary, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "file-change", displayPath: input.displayPath, summary: input.summary}) : invalid("INVALID_ENTRY", "File-change entry is invalid");
    case "unknown":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "sourceType"])) return unknownField();
      return isBoundedString(input.sourceType, 1, MAX_SOURCE_TYPE_LENGTH) ? valid({...base.value, kind: "unknown", sourceType: input.sourceType}) : invalid("INVALID_ENTRY", "Unknown entry is invalid");
    default:
      return invalid("INVALID_ENTRY_KIND", "Session entry kind is invalid");
  }
}

function parseEntryBase(input: JsonRecord): ParseResult<EntryBaseV1> {
  const entryKey = parseOpaqueId(input.entryKey);
  if (!entryKey.ok) return entryKey;
  if (!isIntegerBetween(input.ordinal, 0, MAX_PRESENTATION_ENTRY_COUNT - 1) || !isIntegerBetween(input.atMs, 0, MAX_SOURCE_DURATION_MS)) return invalid("INVALID_ENTRY", "Entry order or timestamp is invalid");
  return valid({entryKey: entryKey.value, ordinal: input.ordinal, atMs: input.atMs});
}

function parseToolDetail(input: unknown): ParseResult<ToolDetailV1> {
  if (!isRecord(input)) return invalid("INVALID_TOOL_DETAIL", "Tool detail is invalid");
  if (input.availability === "unavailable") {
    return hasOnlyKeys(input, ["availability"]) ? valid({availability: "unavailable"}) : unknownField();
  }
  if (input.availability !== "available") return invalid("INVALID_TOOL_DETAIL", "Tool-detail availability is invalid");
  if (!hasOnlyKeys(input, ["availability", "arguments", "result"])) return unknownField();
  if (!isNullableBoundedString(input.arguments, MAX_ENTRY_TEXT_LENGTH) || !isNullableBoundedString(input.result, MAX_ENTRY_TEXT_LENGTH) || (input.arguments === null && input.result === null)) return invalid("INVALID_TOOL_DETAIL", "Available tool detail must contain arguments or a result");
  return valid({availability: "available", arguments: input.arguments, result: input.result});
}

function parsePreferences(input: unknown): ParseResult<SessionPreferencesV1> {
  const record = parseRecord(input, ["schemaVersion", "visibility", "timing", "appearance"], "INVALID_PREFERENCES");
  if (!record.ok) return record;
  if (record.value.schemaVersion !== 1) return invalid("UNSUPPORTED_SCHEMA_VERSION", "Schema version is not supported");
  const visibility = parseVisibility(record.value.visibility);
  if (!visibility.ok) return visibility;
  const timing = parseTiming(record.value.timing);
  if (!timing.ok) return timing;
  const appearance = parseAppearance(record.value.appearance);
  if (!appearance.ok) return appearance;
  return valid({schemaVersion: 1, visibility: visibility.value, timing: timing.value, appearance: appearance.value});
}

function parseVisibility(input: unknown): ParseResult<VisibilityPreferencesV1> {
  const record = parseRecord(input, ["showToolCalls", "showToolDetails", "showReasoning"], "INVALID_VISIBILITY");
  if (!record.ok) return record;
  if (typeof record.value.showToolCalls !== "boolean" || typeof record.value.showToolDetails !== "boolean" || typeof record.value.showReasoning !== "boolean" || (!record.value.showToolCalls && record.value.showToolDetails)) return invalid("INVALID_VISIBILITY", "Visibility preferences are invalid");
  return valid({showToolCalls: record.value.showToolCalls, showToolDetails: record.value.showToolDetails, showReasoning: record.value.showReasoning});
}

function parseTiming(input: unknown): ParseResult<PresentationTimingV1> {
  const record = parseRecord(input, ["entryDelayMs", "playbackSpeed"], "INVALID_PRESENTATION_TIMING");
  if (!record.ok) return record;
  if (!isIntegerBetween(record.value.entryDelayMs, MIN_PRESENTATION_DELAY_MS, MAX_PRESENTATION_DELAY_MS) || !isFiniteBetween(record.value.playbackSpeed, MIN_PRESENTATION_SPEED, MAX_PRESENTATION_SPEED)) return invalid("INVALID_PRESENTATION_TIMING", "Presentation timing is invalid");
  return valid({entryDelayMs: record.value.entryDelayMs, playbackSpeed: record.value.playbackSpeed});
}

function parseAppearance(input: unknown): ParseResult<AppearancePreferencesV1> {
  const record = parseRecord(input, ["theme", "font"], "INVALID_APPEARANCE");
  if (!record.ok) return record;
  const theme = parseTheme(record.value.theme);
  const font = parseFont(record.value.font);
  if (!theme.ok || !font.ok) return invalid("INVALID_APPEARANCE", "Appearance preferences are invalid");
  return valid({theme: theme.value, font: font.value});
}

function parseTheme(input: unknown): ParseResult<ThemeSpec> {
  const record = parseRecord(input, ["background", "surface", "text", "muted", "accent", "success", "error"], "INVALID_APPEARANCE");
  if (!record.ok) return record;
  const {background, surface, text, muted, accent, success, error} = record.value;
  if (!isHexColor(background) || !isHexColor(surface) || !isHexColor(text) || !isHexColor(muted) || !isHexColor(accent) || !isHexColor(success) || !isHexColor(error)) return invalid("INVALID_APPEARANCE", "Theme colors are invalid");
  return valid({background, surface, text, muted, accent, success, error});
}

function parseFont(input: unknown): ParseResult<FontSpec> {
  const record = parseRecord(input, ["family", "sizePx", "lineHeight"], "INVALID_APPEARANCE");
  if (!record.ok) return record;
  if (record.value.family !== "JetBrains Mono" || !isIntegerBetween(record.value.sizePx, 12, 96) || !isFiniteBetween(record.value.lineHeight, 1, 2.5)) return invalid("INVALID_APPEARANCE", "Font settings are invalid");
  return valid({family: record.value.family, sizePx: record.value.sizePx, lineHeight: record.value.lineHeight});
}

function parseContentAvailability(input: unknown): ParseResult<ContentAvailabilityV1> {
  const record = parseRecord(input, ["reasoning", "toolDetails"], "INVALID_CONTENT_AVAILABILITY");
  if (!record.ok) return record;
  if ((record.value.reasoning !== "available" && record.value.reasoning !== "unavailable") || (record.value.toolDetails !== "available" && record.value.toolDetails !== "partial" && record.value.toolDetails !== "unavailable")) return invalid("INVALID_CONTENT_AVAILABILITY", "Content availability is invalid");
  return valid({reasoning: record.value.reasoning, toolDetails: record.value.toolDetails});
}

function parseSelectionChange(input: unknown): ParseResult<EntrySelectionChangeV1> {
  const record = parseRecord(input, ["entryKey", "selected"], "INVALID_SELECTION_CHANGE");
  if (!record.ok) return record;
  const entryKey = parseOpaqueId(record.value.entryKey);
  if (!entryKey.ok) return entryKey;
  if (typeof record.value.selected !== "boolean") return invalid("INVALID_SELECTION", "Selection value is invalid");
  return valid({entryKey: entryKey.value, selected: record.value.selected});
}

function parseRefreshCounts(input: JsonRecord): ParseResult<RefreshCountsV1> {
  const {discoveredCount, processedCount, indexedCount, unchangedCount, failedCount, skippedCount, warningCount} = input;
  if (!isSafeCount(discoveredCount) || !isSafeCount(processedCount) || !isSafeCount(indexedCount) || !isSafeCount(unchangedCount) || !isSafeCount(failedCount) || !isSafeCount(skippedCount) || !isSafeCount(warningCount) || processedCount > discoveredCount || indexedCount + unchangedCount + failedCount + skippedCount !== processedCount) return invalid("INVALID_REFRESH_COUNTS", "Refresh counts are inconsistent");
  return valid({discoveredCount, processedCount, indexedCount, unchangedCount, failedCount, skippedCount, warningCount});
}

function isListCursor(value: unknown, sortOrder: IndexedSessionSortOrderV1): value is string {
  return typeof value === "string" && value.startsWith(`${sortOrder}__`) && isOpaqueId(value.slice(sortOrder.length + 2));
}

function parsePresentationPlanEntry(input: unknown): ParseResult<PresentationPlanEntryV1> {
  const record = parseRecord(input, ["entry", "revealAtMs"], "INVALID_PRESENTATION_ENTRY");
  if (!record.ok) return record;
  const entry = parsePresentationEntryContent(record.value.entry);
  if (!entry.ok) return entry;
  if (!isIntegerBetween(record.value.revealAtMs, 0, MAX_PRESENTATION_DURATION_MS)) return invalid("INVALID_PRESENTATION_ENTRY", "Presentation reveal time is invalid");
  return valid({entry: entry.value, revealAtMs: record.value.revealAtMs});
}

function parsePresentationEntryContent(input: unknown): ParseResult<PresentationEntryContentV1> {
  if (!isRecord(input)) return invalid("INVALID_PRESENTATION_ENTRY", "Presentation entry is invalid");
  const base = parseEntryBase(input);
  if (!base.ok) return base;
  switch (input.kind) {
    case "user":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "text"])) return unknownField();
      return isBoundedString(input.text, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "user", text: input.text}) : invalid("INVALID_PRESENTATION_ENTRY", "User presentation entry is invalid");
    case "assistant":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "markdown"])) return unknownField();
      return isBoundedString(input.markdown, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "assistant", markdown: input.markdown}) : invalid("INVALID_PRESENTATION_ENTRY", "Assistant presentation entry is invalid");
    case "reasoning":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "text"])) return unknownField();
      return isBoundedString(input.text, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "reasoning", text: input.text}) : invalid("INVALID_PRESENTATION_ENTRY", "Reasoning presentation entry is invalid");
    case "tool-call": {
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "name", "status", "summary", "detail"])) return unknownField();
      const detail = parsePresentedToolDetail(input.detail);
      if (!detail.ok) return detail;
      if (!isBoundedString(input.name, 1, MAX_TOOL_NAME_LENGTH) || !isToolStatus(input.status) || !isBoundedString(input.summary, 0, MAX_ENTRY_TEXT_LENGTH)) return invalid("INVALID_PRESENTATION_ENTRY", "Tool presentation entry is invalid");
      return valid({...base.value, kind: "tool-call", name: input.name, status: input.status, summary: input.summary, detail: detail.value});
    }
    case "file-change":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "displayPath", "summary"])) return unknownField();
      return isBoundedString(input.displayPath, 1, MAX_DISPLAY_PATH_LENGTH) && isBoundedString(input.summary, 0, MAX_ENTRY_TEXT_LENGTH) ? valid({...base.value, kind: "file-change", displayPath: input.displayPath, summary: input.summary}) : invalid("INVALID_PRESENTATION_ENTRY", "File-change presentation entry is invalid");
    case "unknown":
      if (!hasOnlyKeys(input, ["entryKey", "ordinal", "atMs", "kind", "sourceType"])) return unknownField();
      return isBoundedString(input.sourceType, 1, MAX_SOURCE_TYPE_LENGTH) ? valid({...base.value, kind: "unknown", sourceType: input.sourceType}) : invalid("INVALID_PRESENTATION_ENTRY", "Unknown presentation entry is invalid");
    default:
      return invalid("INVALID_ENTRY_KIND", "Presentation entry kind is invalid");
  }
}

function parsePresentedToolDetail(input: unknown): ParseResult<PresentedToolDetailV1 | null> {
  if (input === null) return valid(null);
  const record = parseRecord(input, ["arguments", "result"], "INVALID_TOOL_DETAIL");
  if (!record.ok) return record;
  if (!isNullableBoundedString(record.value.arguments, MAX_ENTRY_TEXT_LENGTH) || !isNullableBoundedString(record.value.result, MAX_ENTRY_TEXT_LENGTH) || (record.value.arguments === null && record.value.result === null)) return invalid("INVALID_TOOL_DETAIL", "Presented tool detail is invalid");
  return valid({arguments: record.value.arguments, result: record.value.result});
}

function parseRecord(input: unknown, keys: readonly string[], invalidCode: string): ParseResult<JsonRecord> {
  if (!isRecord(input)) return invalid(invalidCode, "Contract object is invalid");
  if (!hasOnlyKeys(input, keys)) return unknownField();
  return valid(input);
}

function parseOpaqueId(input: unknown): ParseResult<string> {
  return isOpaqueId(input) ? valid(input) : invalid("INVALID_OPAQUE_ID", "Opaque identifier is invalid");
}

function parseConfirmationToken(input: unknown): ParseResult<string> {
  return isOpaqueId(input) ? valid(input) : invalid("INVALID_CONFIRMATION_TOKEN", "Deletion confirmation token is invalid");
}

function isOpaqueId(value: unknown): value is string {
  return typeof value === "string" && value.length <= MAX_OPAQUE_ID_LENGTH && /^[A-Za-z0-9_-]+$/.test(value);
}

function isSafeCode(value: unknown): value is string {
  return typeof value === "string" && value.length <= 128 && /^[A-Z][A-Z0-9_]*$/.test(value);
}

function isSessionSource(value: unknown): value is IndexedSessionSourceV1 {
  return value === "claude-code" || value === "codex" || value === "copilot-cli" || value === "vscode-copilot";
}

function isToolStatus(value: unknown): value is Extract<SessionEntryV1, {kind: "tool-call"}>["status"] {
  return value === "pending" || value === "running" || value === "succeeded" || value === "failed";
}

function isTimestamp(value: unknown): value is number {
  return isIntegerBetween(value, 0, Number.MAX_SAFE_INTEGER);
}

function isSafeCount(value: unknown): value is number {
  return isIntegerBetween(value, 0, MAX_PRESENTATION_ENTRY_COUNT);
}

function isIntegerBetween(value: unknown, minimum: number, maximum: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= minimum && value <= maximum;
}

function isFiniteBetween(value: unknown, minimum: number, maximum: number): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= minimum && value <= maximum;
}

function isNullableBoundedString(value: unknown, maximum: number): value is string | null {
  return value === null || isBoundedString(value, 0, maximum);
}

function isBoundedString(value: unknown, minimum: number, maximum: number): value is string {
  if (typeof value !== "string") return false;
  let length = 0;
  for (const character of value) {
    const codePoint = character.charCodeAt(0);
    if (character === "\0" || (character.length === 1 && codePoint >= 0xd800 && codePoint <= 0xdfff) || ++length > maximum) return false;
  }
  return length >= minimum;
}

function isHexColor(value: unknown): value is string {
  return typeof value === "string" && /^#[0-9A-Fa-f]{6}$/.test(value);
}

function isRecord(input: unknown): input is JsonRecord {
  return typeof input === "object" && input !== null && !Array.isArray(input);
}

function hasOnlyKeys(input: JsonRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(input).every((key) => allowed.has(key));
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
