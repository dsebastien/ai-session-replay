import {invoke} from "@tauri-apps/api/core";
import {listen} from "@tauri-apps/api/event";
import {beforeEach, describe, expect, it, vi} from "vitest";
import contractFixture from "../../../tests/fixtures/indexed-library-contracts-v1.json";
import type {
  IndexedSessionDetailV1,
  IndexedSessionPageV1,
  IndexRefreshStateV1,
  SourceDeletionConfirmationV1,
  SuppressedSourcePageV1,
  SuppressedSourceV1,
} from "../../../packages/replay-contract/src";
import {
  deleteIndexedSession,
  deleteIndexedSessionWithSource,
  createPresentationPlan,
  exportPresentation,
  getIndexedSession,
  getIndexRefreshState,
  getJetBrainsCopilotStatus,
  listSuppressedSources,
  listIndexedSessions,
  prepareSourceDeletion,
  refreshIndex,
  revealExport,
  resetLocalDatabase,
  renameIndexedSession,
  restoreSuppressedSource,
  setEntrySelections,
  setSessionPreferences,
  subscribeToExportProgress,
  subscribeToIndexRefresh,
} from "./indexed-sessions";

vi.mock("@tauri-apps/api/core", () => ({invoke: vi.fn()}));
vi.mock("@tauri-apps/api/event", () => ({listen: vi.fn()}));

const summary = {
  schemaVersion: 1,
  sessionId: "session_1",
  source: "codex",
  title: "Indexed session",
  createdAtMs: 1_000,
  lastIndexedAtMs: 2_000,
  sourcePresent: false,
  entryCount: 1,
  selectedEntryCount: 1,
  durationMs: 500,
  diagnosticCount: 0,
  contentAvailability: {reasoning: "unavailable", toolDetails: "unavailable"},
} as const;

const page: IndexedSessionPageV1 = {
  schemaVersion: 1,
  sortOrder: "newest",
  items: [summary],
  nextCursor: null,
};

const detail: IndexedSessionDetailV1 = {
  schemaVersion: 1,
  summary,
  revision: {
    schemaVersion: 1,
    revisionId: "revision_1",
    sessionId: "session_1",
    indexedAtMs: 2_000,
    entryCount: 1,
    durationMs: 500,
    diagnosticCount: 0,
  },
  entryPage: {
    schemaVersion: 1,
    sessionId: "session_1",
    revisionId: "revision_1",
    entries: [
      {
        entry: {entryKey: "entry_1", ordinal: 0, atMs: 0, kind: "user", text: "Hello"},
        selected: true,
      },
    ],
    nextCursor: null,
    totalEntryCount: 1,
  },
  preferences: {
    schemaVersion: 1,
    visibility: {showToolCalls: true, showToolDetails: false, showReasoning: true},
    timing: {entryDelayMs: 1_000, playbackSpeed: 1},
    appearance: {
      theme: {
        background: "#101211",
        surface: "#171A18",
        text: "#F5F5F4",
        muted: "#9B9E9C",
        accent: "#D6AA68",
        success: "#A8D5BD",
        error: "#E59A91",
      },
      font: {family: "JetBrains Mono", sizePx: 34, lineHeight: 1.5},
    },
  },
};

const tombstone: SuppressedSourceV1 = {
  schemaVersion: 1,
  suppressionId: "suppression_1",
  source: "codex",
  sourceDeleted: false,
  suppressedAtMs: 3_000,
};

const confirmation: SourceDeletionConfirmationV1 = {
  schemaVersion: 1,
  sessionId: "session_1",
  confirmationToken: "delete_token_1",
  artifactCount: 1,
  expiresAtMs: 303_000,
};

const suppressedPage: SuppressedSourcePageV1 = {
  schemaVersion: 1,
  items: [tombstone],
  nextCursor: null,
};

describe("indexed session client", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(listen).mockReset();
  });

  it("uses only bounded path-free list and detail commands", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(page).mockResolvedValueOnce(detail);

    await expect(
      listIndexedSessions({
        schemaVersion: 1,
        source: "codex",
        query: "indexed",
        sortOrder: "newest",
        cursor: null,
        pageSize: 40,
      }),
    ).resolves.toEqual(page);
    await expect(getIndexedSession("session_1", null)).resolves.toEqual(detail);

    expect(invoke).toHaveBeenNthCalledWith(1, "list_indexed_sessions", {
      request: {
        schemaVersion: 1,
        source: "codex",
        query: "indexed",
        sortOrder: "newest",
        cursor: null,
        pageSize: 40,
      },
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "get_indexed_session", {
      sessionId: "session_1",
      entryCursor: null,
    });
    expect(JSON.stringify(vi.mocked(invoke).mock.calls)).not.toContain("path");
  });

  it("rejects invalid outgoing requests before IPC", async () => {
    await expect(
      listIndexedSessions({
        schemaVersion: 1,
        source: null,
        query: "",
        sortOrder: "newest",
        cursor: null,
        pageSize: 201,
      }),
    ).rejects.toThrow("Invalid indexed-session list request");
    await expect(getIndexedSession("../session", null)).rejects.toThrow(
      "Invalid indexed session identifier",
    );
    await expect(getIndexedSession("session_1", "bad/cursor")).rejects.toThrow(
      "Invalid entry cursor",
    );
    await expect(createPresentationPlan("../session", "revision_1")).rejects.toThrow(
      "Invalid presentation request",
    );
    await expect(exportPresentation("../plan", "D:\\out.mp4", "export_1")).rejects.toThrow(
      "Invalid export request",
    );
    expect(invoke).not.toHaveBeenCalled();
  });

  it("rejects malformed command responses without exposing their contents", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({...page, nextCursor: "../private"})
      .mockResolvedValueOnce({...page, sortOrder: "oldest"})
      .mockResolvedValueOnce({...detail, summary: {...summary, title: ""}});

    await expect(
      listIndexedSessions({
        schemaVersion: 1,
        source: null,
        query: "",
        sortOrder: "newest",
        cursor: null,
        pageSize: 40,
      }),
    ).rejects.toThrow("Invalid indexed-session page response");
    await expect(
      listIndexedSessions({
        schemaVersion: 1,
        source: null,
        query: "",
        sortOrder: "newest",
        cursor: null,
        pageSize: 40,
      }),
    ).rejects.toThrow("Invalid indexed-session page response");
    await expect(getIndexedSession("session_1", null)).rejects.toThrow(
      "Invalid indexed-session detail response",
    );
  });

  it("validates current state, refresh results, and event payloads", async () => {
    const running: IndexRefreshStateV1 = {
      schemaVersion: 1,
      status: "running",
      generation: 3,
      startedAtMs: 1_000,
      discoveredCount: 2,
      processedCount: 1,
      indexedCount: 1,
      unchangedCount: 0,
      failedCount: 0,
      skippedCount: 0,
      warningCount: 0,
    };
    const unlisten = vi.fn();
    let eventHandler: ((event: {payload: unknown}) => void) | undefined;
    vi.mocked(invoke).mockResolvedValueOnce(running).mockResolvedValueOnce(running);
    vi.mocked(listen).mockImplementationOnce(async (_event, handler) => {
      eventHandler = handler as (event: {payload: unknown}) => void;
      return unlisten;
    });
    const observer = vi.fn();

    await expect(getIndexRefreshState()).resolves.toEqual(running);
    await expect(refreshIndex()).resolves.toEqual(running);
    const unsubscribe = await subscribeToIndexRefresh(observer);
    eventHandler?.({payload: running});
    eventHandler?.({payload: {...running, processedCount: 3}});

    expect(invoke).toHaveBeenNthCalledWith(1, "get_index_refresh_state");
    expect(invoke).toHaveBeenNthCalledWith(2, "refresh_index");
    expect(listen).toHaveBeenCalledWith("index-refresh-state-v1", expect.any(Function));
    expect(observer).toHaveBeenCalledTimes(1);
    expect(observer).toHaveBeenCalledWith(running);
    unsubscribe();
    expect(unlisten).toHaveBeenCalledTimes(1);

    vi.mocked(invoke).mockResolvedValueOnce({...running, generation: 0});
    await expect(refreshIndex()).rejects.toThrow("Invalid index refresh response");
  });

  it("loads only the path-free JetBrains integration status", async () => {
    const status = {
      schemaVersion: 1,
      pluginStatus: "detected",
      nativeTranscriptStatus: "unsupported",
      copilotCliStatus: "available-separately",
    } as const;
    vi.mocked(invoke).mockResolvedValueOnce(status);

    await expect(getJetBrainsCopilotStatus()).resolves.toEqual(status);
    expect(invoke).toHaveBeenCalledWith("get_jetbrains_copilot_status");

    vi.mocked(invoke).mockResolvedValueOnce({...status, path: "C:\\private"});
    await expect(getJetBrainsCopilotStatus()).rejects.toThrow(
      "Invalid JetBrains Copilot status response",
    );
  });

  it("uses distinct validated commands for library and source deletion", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(tombstone)
      .mockResolvedValueOnce(confirmation)
      .mockResolvedValueOnce({...tombstone, sourceDeleted: true});

    await expect(deleteIndexedSession("session_1")).resolves.toEqual(tombstone);
    await expect(prepareSourceDeletion("session_1")).resolves.toEqual(confirmation);
    await expect(
      deleteIndexedSessionWithSource("session_1", "delete_token_1"),
    ).resolves.toEqual({...tombstone, sourceDeleted: true});

    expect(invoke).toHaveBeenNthCalledWith(1, "delete_indexed_session", {
      request: {schemaVersion: 1, mode: "library-only", sessionId: "session_1"},
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "prepare_source_deletion", {
      request: {schemaVersion: 1, mode: "prepare-source-deletion", sessionId: "session_1"},
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "delete_indexed_session_with_source", {
      request: {
        schemaVersion: 1,
        mode: "library-and-source",
        sessionId: "session_1",
        confirmationToken: "delete_token_1",
      },
    });
    expect(JSON.stringify(vi.mocked(invoke).mock.calls)).not.toContain("path");
  });

  it("lists and restores suppressed sources through opaque identifiers", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(suppressedPage)
      .mockResolvedValueOnce({schemaVersion: 1, suppressionId: "suppression_1"});

    await expect(
      listSuppressedSources({schemaVersion: 1, cursor: null, pageSize: 40}),
    ).resolves.toEqual(suppressedPage);
    await expect(restoreSuppressedSource("suppression_1")).resolves.toEqual({
      schemaVersion: 1,
      suppressionId: "suppression_1",
    });

    expect(invoke).toHaveBeenNthCalledWith(1, "list_suppressed_sources", {
      request: {schemaVersion: 1, cursor: null, pageSize: 40},
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "restore_suppressed_source", {
      request: {schemaVersion: 1, suppressionId: "suppression_1"},
    });
  });

  it("resets only through the typed path-free database command", async () => {
    const result = {schemaVersion: 1, resetAtMs: 4_000} as const;
    vi.mocked(invoke).mockResolvedValueOnce(result);

    await expect(resetLocalDatabase()).resolves.toEqual(result);
    expect(invoke).toHaveBeenCalledWith("reset_local_database", {
      request: {schemaVersion: 1, mode: "reset-local-database"},
    });

    vi.mocked(invoke).mockResolvedValueOnce({...result, databasePath: "C:\\private"});
    await expect(resetLocalDatabase()).rejects.toThrow("Invalid database reset response");
  });

  it("writes curation through revision-checked typed commands", async () => {
    vi.mocked(invoke).mockResolvedValue(detail);
    const selectionRequest = {
      schemaVersion: 1,
      sessionId: "session_1",
      revisionId: "revision_1",
      changes: [{entryKey: "entry_1", selected: false}],
    } as const;
    const preferencesRequest = {
      schemaVersion: 1,
      sessionId: "session_1",
      revisionId: "revision_1",
      preferences: detail.preferences,
    } as const;

    await expect(setEntrySelections(selectionRequest)).resolves.toEqual(detail);
    await expect(setSessionPreferences(preferencesRequest)).resolves.toEqual(detail);
    expect(invoke).toHaveBeenNthCalledWith(1, "set_entry_selections", {
      request: selectionRequest,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "set_session_preferences", {
      request: preferencesRequest,
    });
  });

  it("renames a session through the path-free local-library command", async () => {
    vi.mocked(invoke).mockResolvedValue(detail);
    const request = {
      schemaVersion: 1,
      sessionId: "session_1",
      title: "Renamed locally",
    } as const;

    await expect(renameIndexedSession(request)).resolves.toEqual(detail);
    expect(invoke).toHaveBeenCalledWith("rename_indexed_session", {request});
    expect(JSON.stringify(vi.mocked(invoke).mock.calls)).not.toContain("path");
  });

  it("creates and exports a backend-frozen presentation plan through opaque IDs", async () => {
    const media = {codec: "h264", width: 1_920, height: 1_080, pixelFormat: "yuv420p", durationMs: 5_000, frameCount: 150};
    vi.mocked(invoke)
      .mockResolvedValueOnce(contractFixture.presentationPlan)
      .mockResolvedValueOnce(media)
      .mockResolvedValueOnce(undefined);

    const plan = await createPresentationPlan("session_01", "revision_01");
    await expect(exportPresentation(plan.planId, "D:\\exports\\replay.mp4", "export_1"))
      .resolves.toEqual(media);
    expect(invoke).toHaveBeenNthCalledWith(1, "create_presentation_plan", {
      sessionId: "session_01",
      revisionId: "revision_01",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "export_presentation", expect.objectContaining({
      planId: "plan_01",
      outputPath: "D:\\exports\\replay.mp4",
      jobId: "export_1",
    }));

    await expect(revealExport("export_1")).resolves.toBeUndefined();
    expect(invoke).toHaveBeenNthCalledWith(3, "reveal_export", {jobId: "export_1"});
  });

  it("accepts only bounded progress for the requested export job", async () => {
    const observer = vi.fn();
    let eventHandler: ((event: {payload: unknown}) => void) | undefined;
    vi.mocked(listen).mockImplementationOnce(async (_event, handler) => {
      eventHandler = handler as (event: {payload: unknown}) => void;
      return () => undefined;
    });

    await subscribeToExportProgress("export_1", observer);
    eventHandler?.({payload: {schemaVersion: 1, jobId: "export_1", renderedFrames: 30, totalFrames: 60}});
    eventHandler?.({payload: {schemaVersion: 1, jobId: "export_2", renderedFrames: 40, totalFrames: 60}});
    eventHandler?.({payload: {schemaVersion: 1, jobId: "export_1", renderedFrames: 61, totalFrames: 60}});

    expect(listen).toHaveBeenCalledWith("export-progress-v1", expect.any(Function));
    expect(observer).toHaveBeenCalledTimes(1);
    expect(observer).toHaveBeenCalledWith({
      schemaVersion: 1,
      jobId: "export_1",
      renderedFrames: 30,
      totalFrames: 60,
    });
  });

  it("rejects invalid deletion requests and path-bearing responses before use", async () => {
    await expect(deleteIndexedSession("../session")).rejects.toThrow(
      "Invalid indexed-session deletion request",
    );
    await expect(
      deleteIndexedSessionWithSource("session_1", "bad/token"),
    ).rejects.toThrow("Invalid source-deletion request");
    await expect(
      listSuppressedSources({schemaVersion: 1, cursor: null, pageSize: 201}),
    ).rejects.toThrow("Invalid suppressed-source list request");
    await expect(restoreSuppressedSource("../private")).rejects.toThrow(
      "Invalid suppression identifier",
    );
    expect(invoke).not.toHaveBeenCalled();

    vi.mocked(invoke)
      .mockResolvedValueOnce({...confirmation, path: "C:\\private\\session.jsonl"})
      .mockResolvedValueOnce({...tombstone, suppressionId: "../private"})
      .mockResolvedValueOnce({...suppressedPage, nextCursor: "../private"})
      .mockResolvedValueOnce({schemaVersion: 1, suppressionId: "../private"});
    await expect(prepareSourceDeletion("session_1")).rejects.toThrow(
      "Invalid source-deletion confirmation response",
    );
    await expect(deleteIndexedSession("session_1")).rejects.toThrow(
      "Invalid indexed-session deletion response",
    );
    await expect(
      listSuppressedSources({schemaVersion: 1, cursor: null, pageSize: 40}),
    ).rejects.toThrow("Invalid suppressed-source page response");
    await expect(restoreSuppressedSource("suppression_1")).rejects.toThrow(
      "Invalid suppression restore response",
    );
  });
});
