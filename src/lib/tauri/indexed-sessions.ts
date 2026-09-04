import {invoke} from "@tauri-apps/api/core";
import {listen, type UnlistenFn} from "@tauri-apps/api/event";
import {
  validateDeleteIndexedSessionRequest,
  validateDeleteIndexedSessionWithSourceRequest,
  validateIndexedSessionDetail,
  validateIndexedSessionListRequest,
  validateIndexedSessionPage,
  validateIndexRefreshState,
  validateJetBrainsCopilotStatus,
  validatePrepareSourceDeletionRequest,
  validatePresentationPlan,
  validateResetLocalDatabaseRequest,
  validateResetLocalDatabaseResult,
  validateRenameIndexedSessionRequest,
  validateRestoreSuppressedSourceRequest,
  validateRestoreSuppressedSourceResult,
  validateSourceDeletionConfirmation,
  validateSetEntrySelectionsRequest,
  validateSetSessionPreferencesRequest,
  validateSuppressedSource,
  validateSuppressedSourceListRequest,
  validateSuppressedSourcePage,
  type DeleteIndexedSessionRequestV1,
  type DeleteIndexedSessionWithSourceRequestV1,
  type IndexedSessionDetailV1,
  type IndexedSessionListRequestV1,
  type IndexedSessionPageV1,
  type IndexRefreshStateV1,
  type JetBrainsCopilotStatusV1,
  type PrepareSourceDeletionRequestV1,
  type ResetLocalDatabaseRequestV1,
  type ResetLocalDatabaseResultV1,
  type RenameIndexedSessionRequestV1,
  type SetEntrySelectionsRequestV1,
  type SetSessionPreferencesRequestV1,
  type RestoreSuppressedSourceRequestV1,
  type RestoreSuppressedSourceResultV1,
  type SourceDeletionConfirmationV1,
  type SuppressedSourceListRequestV1,
  type SuppressedSourcePageV1,
  type SuppressedSourceV1,
} from "../../../packages/replay-contract/src";

export interface IndexedSessionClient {
  listSessions(request: IndexedSessionListRequestV1): Promise<IndexedSessionPageV1>;
  getSession(sessionId: string, entryCursor: string | null): Promise<IndexedSessionDetailV1>;
  refresh(): Promise<IndexRefreshStateV1>;
  currentRefreshState(): Promise<IndexRefreshStateV1>;
  subscribeToRefresh(observer: (state: IndexRefreshStateV1) => void): Promise<UnlistenFn>;
  getJetBrainsStatus(): Promise<JetBrainsCopilotStatusV1>;
  deleteSession(sessionId: string): Promise<SuppressedSourceV1>;
  prepareSourceDeletion(sessionId: string): Promise<SourceDeletionConfirmationV1>;
  deleteSessionWithSource(
    sessionId: string,
    confirmationToken: string,
  ): Promise<SuppressedSourceV1>;
  listSuppressed(request: SuppressedSourceListRequestV1): Promise<SuppressedSourcePageV1>;
  restoreSuppressed(suppressionId: string): Promise<RestoreSuppressedSourceResultV1>;
  resetLocalDatabase(): Promise<ResetLocalDatabaseResultV1>;
  renameSession(request: RenameIndexedSessionRequestV1): Promise<IndexedSessionDetailV1>;
  setEntrySelections(request: SetEntrySelectionsRequestV1): Promise<IndexedSessionDetailV1>;
  setSessionPreferences(request: SetSessionPreferencesRequestV1): Promise<IndexedSessionDetailV1>;
  createPresentationPlan(sessionId: string, revisionId: string): Promise<import("../../../packages/replay-contract/src").PresentationPlanV1>;
  exportPresentation(planId: string, outputPath: string, jobId: string): Promise<ExportMediaSummary>;
  cancelExport(jobId: string): Promise<void>;
  revealExport(jobId: string): Promise<void>;
  subscribeToExportProgress(
    jobId: string,
    observer: (progress: ExportProgressV1) => void,
  ): Promise<UnlistenFn>;
}

export interface ExportMediaSummary {
  readonly codec: string;
  readonly width: number;
  readonly height: number;
  readonly pixelFormat: string;
  readonly durationMs: number;
  readonly frameCount: number;
}

export interface ExportProgressV1 {
  readonly schemaVersion: 1;
  readonly jobId: string;
  readonly renderedFrames: number;
  readonly totalFrames: number;
}

const INDEX_REFRESH_EVENT = "index-refresh-state-v1";
const EXPORT_PROGRESS_EVENT = "export-progress-v1";

export async function listIndexedSessions(
  request: IndexedSessionListRequestV1,
): Promise<IndexedSessionPageV1> {
  if (!validateIndexedSessionListRequest(request).ok) {
    throw new Error("Invalid indexed-session list request");
  }
  const parsed = validateIndexedSessionPage(
    await invoke<unknown>("list_indexed_sessions", {request}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid indexed-session page response");
  }
  if (parsed.value.sortOrder !== request.sortOrder) {
    throw new Error("Invalid indexed-session page response");
  }
  return parsed.value;
}

export async function getIndexedSession(
  sessionId: string,
  entryCursor: string | null,
): Promise<IndexedSessionDetailV1> {
  if (!isOpaqueId(sessionId)) {
    throw new Error("Invalid indexed session identifier");
  }
  if (entryCursor !== null && !isOpaqueId(entryCursor)) {
    throw new Error("Invalid entry cursor");
  }
  const parsed = validateIndexedSessionDetail(
    await invoke<unknown>("get_indexed_session", {sessionId, entryCursor}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid indexed-session detail response");
  }
  return parsed.value;
}

export async function refreshIndex(): Promise<IndexRefreshStateV1> {
  const parsed = validateIndexRefreshState(await invoke<unknown>("refresh_index"));
  if (!parsed.ok) {
    throw new Error("Invalid index refresh response");
  }
  return parsed.value;
}

export async function getIndexRefreshState(): Promise<IndexRefreshStateV1> {
  const parsed = validateIndexRefreshState(
    await invoke<unknown>("get_index_refresh_state"),
  );
  if (!parsed.ok) {
    throw new Error("Invalid index refresh response");
  }
  return parsed.value;
}

export async function getJetBrainsCopilotStatus(): Promise<JetBrainsCopilotStatusV1> {
  const parsed = validateJetBrainsCopilotStatus(
    await invoke<unknown>("get_jetbrains_copilot_status"),
  );
  if (!parsed.ok) {
    throw new Error("Invalid JetBrains Copilot status response");
  }
  return parsed.value;
}

export async function subscribeToIndexRefresh(
  observer: (state: IndexRefreshStateV1) => void,
): Promise<UnlistenFn> {
  return listen<unknown>(INDEX_REFRESH_EVENT, ({payload}) => {
    const parsed = validateIndexRefreshState(payload);
    if (parsed.ok) {
      observer(parsed.value);
    }
  });
}

export async function deleteIndexedSession(sessionId: string): Promise<SuppressedSourceV1> {
  const request: DeleteIndexedSessionRequestV1 = {
    schemaVersion: 1,
    mode: "library-only",
    sessionId,
  };
  if (!validateDeleteIndexedSessionRequest(request).ok) {
    throw new Error("Invalid indexed-session deletion request");
  }
  const parsed = validateSuppressedSource(
    await invoke<unknown>("delete_indexed_session", {request}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid indexed-session deletion response");
  }
  return parsed.value;
}

export async function prepareSourceDeletion(
  sessionId: string,
): Promise<SourceDeletionConfirmationV1> {
  const request: PrepareSourceDeletionRequestV1 = {
    schemaVersion: 1,
    mode: "prepare-source-deletion",
    sessionId,
  };
  if (!validatePrepareSourceDeletionRequest(request).ok) {
    throw new Error("Invalid source-deletion preparation request");
  }
  const parsed = validateSourceDeletionConfirmation(
    await invoke<unknown>("prepare_source_deletion", {request}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid source-deletion confirmation response");
  }
  return parsed.value;
}

export async function deleteIndexedSessionWithSource(
  sessionId: string,
  confirmationToken: string,
): Promise<SuppressedSourceV1> {
  const request: DeleteIndexedSessionWithSourceRequestV1 = {
    schemaVersion: 1,
    mode: "library-and-source",
    sessionId,
    confirmationToken,
  };
  if (!validateDeleteIndexedSessionWithSourceRequest(request).ok) {
    throw new Error("Invalid source-deletion request");
  }
  const parsed = validateSuppressedSource(
    await invoke<unknown>("delete_indexed_session_with_source", {request}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid indexed-session deletion response");
  }
  return parsed.value;
}

export async function listSuppressedSources(
  request: SuppressedSourceListRequestV1,
): Promise<SuppressedSourcePageV1> {
  if (!validateSuppressedSourceListRequest(request).ok) {
    throw new Error("Invalid suppressed-source list request");
  }
  const parsed = validateSuppressedSourcePage(
    await invoke<unknown>("list_suppressed_sources", {request}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid suppressed-source page response");
  }
  return parsed.value;
}

export async function restoreSuppressedSource(
  suppressionId: string,
): Promise<RestoreSuppressedSourceResultV1> {
  const request: RestoreSuppressedSourceRequestV1 = {schemaVersion: 1, suppressionId};
  if (!validateRestoreSuppressedSourceRequest(request).ok) {
    throw new Error("Invalid suppression identifier");
  }
  const parsed = validateRestoreSuppressedSourceResult(
    await invoke<unknown>("restore_suppressed_source", {request}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid suppression restore response");
  }
  return parsed.value;
}

export async function resetLocalDatabase(): Promise<ResetLocalDatabaseResultV1> {
  const request: ResetLocalDatabaseRequestV1 = {
    schemaVersion: 1,
    mode: "reset-local-database",
  };
  if (!validateResetLocalDatabaseRequest(request).ok) {
    throw new Error("Invalid database reset request");
  }
  const parsed = validateResetLocalDatabaseResult(
    await invoke<unknown>("reset_local_database", {request}),
  );
  if (!parsed.ok) {
    throw new Error("Invalid database reset response");
  }
  return parsed.value;
}

export async function setEntrySelections(
  request: SetEntrySelectionsRequestV1,
): Promise<IndexedSessionDetailV1> {
  if (!validateSetEntrySelectionsRequest(request).ok) {
    throw new Error("Invalid entry selection request");
  }
  const parsed = validateIndexedSessionDetail(
    await invoke<unknown>("set_entry_selections", {request}),
  );
  if (!parsed.ok) throw new Error("Invalid entry selection response");
  return parsed.value;
}

export async function renameIndexedSession(
  request: RenameIndexedSessionRequestV1,
): Promise<IndexedSessionDetailV1> {
  if (!validateRenameIndexedSessionRequest(request).ok) {
    throw new Error("Invalid session rename request");
  }
  const parsed = validateIndexedSessionDetail(
    await invoke<unknown>("rename_indexed_session", {request}),
  );
  if (!parsed.ok) throw new Error("Invalid session rename response");
  return parsed.value;
}

export async function setSessionPreferences(
  request: SetSessionPreferencesRequestV1,
): Promise<IndexedSessionDetailV1> {
  if (!validateSetSessionPreferencesRequest(request).ok) {
    throw new Error("Invalid session preferences request");
  }
  const parsed = validateIndexedSessionDetail(
    await invoke<unknown>("set_session_preferences", {request}),
  );
  if (!parsed.ok) throw new Error("Invalid session preferences response");
  return parsed.value;
}

export async function exportPresentation(
  planId: string,
  outputPath: string,
  jobId: string,
): Promise<ExportMediaSummary> {
  if (!isOpaqueId(planId) || typeof outputPath !== "string" || outputPath.length === 0 || !isOpaqueId(jobId)) {
    throw new Error("Invalid export request");
  }
  const result = await invoke<unknown>("export_presentation", {planId, outputPath, jobId});
  if (!isExportMediaSummary(result)) throw new Error("Invalid export response");
  return result;
}

export async function createPresentationPlan(
  sessionId: string,
  revisionId: string,
): Promise<import("../../../packages/replay-contract/src").PresentationPlanV1> {
  if (!isOpaqueId(sessionId) || !isOpaqueId(revisionId)) {
    throw new Error("Invalid presentation request");
  }
  const parsed = validatePresentationPlan(
    await invoke<unknown>("create_presentation_plan", {sessionId, revisionId}),
  );
  if (!parsed.ok) throw new Error("Invalid presentation response");
  return parsed.value;
}

export async function cancelExport(jobId: string): Promise<void> {
  if (!isOpaqueId(jobId)) throw new Error("Invalid export job identifier");
  await invoke("cancel_export", {jobId});
}

export async function revealExport(jobId: string): Promise<void> {
  if (!isOpaqueId(jobId)) throw new Error("Invalid export job identifier");
  await invoke("reveal_export", {jobId});
}

export async function subscribeToExportProgress(
  jobId: string,
  observer: (progress: ExportProgressV1) => void,
): Promise<UnlistenFn> {
  if (!isOpaqueId(jobId)) throw new Error("Invalid export job identifier");
  return listen<unknown>(EXPORT_PROGRESS_EVENT, ({payload}) => {
    if (isExportProgress(payload) && payload.jobId === jobId) observer(payload);
  });
}

export const indexedSessionClient: IndexedSessionClient = {
  listSessions: listIndexedSessions,
  getSession: getIndexedSession,
  refresh: refreshIndex,
  currentRefreshState: getIndexRefreshState,
  subscribeToRefresh: subscribeToIndexRefresh,
  getJetBrainsStatus: getJetBrainsCopilotStatus,
  deleteSession: deleteIndexedSession,
  prepareSourceDeletion,
  deleteSessionWithSource: deleteIndexedSessionWithSource,
  listSuppressed: listSuppressedSources,
  restoreSuppressed: restoreSuppressedSource,
  resetLocalDatabase,
  renameSession: renameIndexedSession,
  setEntrySelections,
  setSessionPreferences,
  createPresentationPlan,
  exportPresentation,
  cancelExport,
  revealExport,
  subscribeToExportProgress,
};

function isExportMediaSummary(value: unknown): value is ExportMediaSummary {
  return typeof value === "object" && value !== null &&
    (value as ExportMediaSummary).codec === "h264" &&
    (value as ExportMediaSummary).width === 1_920 &&
    (value as ExportMediaSummary).height === 1_080 &&
    (value as ExportMediaSummary).pixelFormat === "yuv420p" &&
    Number.isSafeInteger((value as ExportMediaSummary).frameCount) &&
    Number.isSafeInteger((value as ExportMediaSummary).durationMs);
}

function isExportProgress(value: unknown): value is ExportProgressV1 {
  if (typeof value !== "object" || value === null) return false;
  const progress = value as ExportProgressV1;
  return progress.schemaVersion === 1 &&
    isOpaqueId(progress.jobId) &&
    Number.isSafeInteger(progress.renderedFrames) &&
    progress.renderedFrames >= 0 &&
    Number.isSafeInteger(progress.totalFrames) &&
    progress.totalFrames >= 1 &&
    progress.totalFrames <= 216_000 &&
    progress.renderedFrames <= progress.totalFrames;
}

function isOpaqueId(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length >= 1 &&
    value.length <= 256 &&
    /^[A-Za-z0-9_-]+$/.test(value)
  );
}
