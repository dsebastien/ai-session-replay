import {ArchiveRestore, LoaderCircle, RefreshCw, Settings} from "lucide-react";
import {useCallback, useEffect, useRef, useState} from "react";
import {save} from "@tauri-apps/plugin-dialog";
import type {
  IndexedSessionSummaryV1,
  IndexedSessionSortOrderV1,
  IndexRefreshStateV1,
  JetBrainsCopilotStatusV1,
  PresentationPlanV1,
  SessionPreferencesV1,
} from "../../../packages/replay-contract/src";
import {
  indexedSessionClient,
  type IndexedSessionClient,
} from "../../lib/tauri/indexed-sessions";
import {SessionList} from "./SessionList";
import {SessionDeletionDialog} from "./SessionDeletionDialog";
import {LibrarySettingsDialog} from "./LibrarySettingsDialog";
import {PresentationDialog} from "./PresentationDialog";
import {
  SessionSelection,
  type SessionExportState,
  type SessionSelectionState,
} from "./SessionSelection";
import {SourceSidebar, type SessionSourceFilter} from "./SourceSidebar";
import {SuppressedSourcesDialog} from "./SuppressedSourcesDialog";
import {defaultExportFileName, exportFailureMessage, exportWasCancelled} from "./exportFailure";

interface SessionBrowserProps {
  readonly client?: IndexedSessionClient;
}

interface LibraryState {
  readonly entries: readonly IndexedSessionSummaryV1[];
  readonly nextCursor: string | null;
  readonly status: "loading" | "ready" | "error";
  readonly loadingMore: boolean;
}

const PAGE_SIZE = 40;
const INITIAL_LIBRARY_STATE: LibraryState = {
  entries: [],
  nextCursor: null,
  status: "loading",
  loadingMore: false,
};
const INITIAL_REFRESH_STATE: IndexRefreshStateV1 = {
  schemaVersion: 1,
  status: "idle",
  lastCompletedAtMs: null,
};
const INITIAL_EXPORT_STATE: SessionExportState = {
  status: "idle",
  progressPercent: null,
  message: null,
};

export function SessionBrowser({client = indexedSessionClient}: SessionBrowserProps) {
  const [library, setLibrary] = useState<LibraryState>(INITIAL_LIBRARY_STATE);
  const [sourcesCollapsed, setSourcesCollapsed] = useState(false);
  const [selectedSource, setSelectedSource] = useState<SessionSourceFilter>("all");
  const [query, setQuery] = useState("");
  const [sortOrder, setSortOrder] = useState<IndexedSessionSortOrderV1>("newest");
  const [selection, setSelection] = useState<SessionSelectionState>({status: "idle"});
  const [refreshState, setRefreshState] = useState<IndexRefreshStateV1>(INITIAL_REFRESH_STATE);
  const [refreshRequestPending, setRefreshRequestPending] = useState(false);
  const [refreshCommandFailed, setRefreshCommandFailed] = useState(false);
  const [jetBrainsStatus, setJetBrainsStatus] = useState<JetBrainsCopilotStatusV1 | null>(null);
  const [startupReady, setStartupReady] = useState(false);
  const [resetCompleted, setResetCompleted] = useState(false);
  const [reloadToken, setReloadToken] = useState(0);
  const [deletionTarget, setDeletionTarget] = useState<IndexedSessionSummaryV1 | null>(null);
  const [showSuppressed, setShowSuppressed] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [presentationPlan, setPresentationPlan] = useState<PresentationPlanV1 | null>(null);
  const [presenting, setPresenting] = useState(false);
  const [presentationError, setPresentationError] = useState(false);
  const [sessionExport, setSessionExport] = useState<SessionExportState>(INITIAL_EXPORT_STATE);
  const listRequest = useRef(0);
  const detailRequest = useRef(0);
  const presentationRequest = useRef(0);
  const exportRequest = useRef(0);
  const activeExportJob = useRef<string | null>(null);
  const completedExportJob = useRef<string | null>(null);
  const lastCompletedGeneration = useRef(0);
  const detailReloadToken = useRef(0);
  const latestRefreshState = useRef<IndexRefreshStateV1>(INITIAL_REFRESH_STATE);
  const selectionRef = useRef(selection);
  selectionRef.current = selection;

  const acceptRefreshState = useCallback((state: IndexRefreshStateV1) => {
    if (!refreshStateAdvances(latestRefreshState.current, state)) return;
    latestRefreshState.current = state;
    setRefreshState(state);
    setRefreshCommandFailed(false);
    setResetCompleted(false);
    if (
      state.status === "completed" &&
      state.generation > lastCompletedGeneration.current
    ) {
      lastCompletedGeneration.current = state.generation;
      setReloadToken((value) => value + 1);
    }
    if (state.status === "failed") setStartupReady(true);
  }, []);

  const loadFirstPage = useCallback(async () => {
    const requestId = ++listRequest.current;
    setLibrary((current) => ({...current, status: "loading", loadingMore: false}));
    try {
      const page = await client.listSessions({
        schemaVersion: 1,
        source: selectedSource === "all" ? null : selectedSource,
        query: query.trim(),
        sortOrder,
        cursor: null,
        pageSize: PAGE_SIZE,
      });
      if (requestId === listRequest.current) {
        setLibrary({
          entries: page.items,
          nextCursor: page.nextCursor,
          status: "ready",
          loadingMore: false,
        });
        if (
          page.items.length > 0 ||
          latestRefreshState.current.status === "completed" ||
          latestRefreshState.current.status === "failed"
        ) {
          setStartupReady(true);
        }
      }
    } catch {
      if (requestId === listRequest.current) {
        setLibrary((current) => ({...current, status: "error", loadingMore: false}));
        setStartupReady(true);
      }
    }
  }, [client, query, selectedSource, sortOrder]);

  useEffect(() => {
    void loadFirstPage();
  }, [loadFirstPage, reloadToken]);

  useEffect(() => {
    if (reloadToken === 0) return;
    const current = selectionRef.current;
    if (current.status !== "ready" || detailReloadToken.current >= reloadToken) return;

    const sessionId = current.detail.summary.sessionId;
    const requestId = ++detailRequest.current;
    detailReloadToken.current = reloadToken;
    setSelection({...current, refreshing: true, refreshError: false});
    void client
      .getSession(sessionId, null)
      .then((detail) => {
        if (requestId !== detailRequest.current) return;
        setSelection((latest) =>
          latest.status === "ready" && latest.detail.summary.sessionId === sessionId
            ? {
                status: "ready",
                detail,
                entries: detail.entryPage.entries,
                nextCursor: detail.entryPage.nextCursor,
                loadingMore: false,
                refreshing: false,
                refreshError: false,
                pageError: false,
              }
            : latest,
        );
      })
      .catch(() => {
        if (requestId !== detailRequest.current) return;
        setSelection((latest) =>
          latest.status === "ready" && latest.detail.summary.sessionId === sessionId
            ? {...latest, refreshing: false, refreshError: true}
            : latest,
        );
      });
  }, [client, reloadToken, selection.status]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void client
      .subscribeToRefresh(acceptRefreshState)
      .then((unsubscribe) => {
        if (disposed) unsubscribe();
        else unlisten = unsubscribe;
      })
      .catch(() => {
        if (!disposed) {
          setRefreshCommandFailed(true);
          setStartupReady(true);
        }
      });
    void client
      .currentRefreshState()
      .then((state) => {
        if (!disposed) acceptRefreshState(state);
      })
      .catch(() => {
        if (!disposed) {
          setRefreshCommandFailed(true);
          setStartupReady(true);
        }
      });
    return () => {
      disposed = true;
      unlisten?.();
      ++listRequest.current;
      ++detailRequest.current;
    };
  }, [acceptRefreshState, client]);

  useEffect(() => {
    let disposed = false;
    void client
      .getJetBrainsStatus()
      .then((status) => {
        if (!disposed) setJetBrainsStatus(status);
      })
      .catch(() => {
        if (!disposed) {
          setJetBrainsStatus({
            schemaVersion: 1,
            pluginStatus: "unavailable",
            nativeTranscriptStatus: "unsupported",
            copilotCliStatus: "unavailable",
          });
        }
      });
    return () => {
      disposed = true;
    };
  }, [client, reloadToken]);

  useEffect(() => () => {
    ++exportRequest.current;
    const jobId = activeExportJob.current;
    if (jobId) void client.cancelExport(jobId).catch(() => undefined);
  }, [client]);

  const selectSession = useCallback(
    async (summary: IndexedSessionSummaryV1) => {
      ++presentationRequest.current;
      setPresentationError(false);
      setPresenting(false);
      const requestId = ++detailRequest.current;
      const requestReloadToken = reloadToken;
      setSelection({status: "loading", summary});
      try {
        const detail = await client.getSession(summary.sessionId, null);
        if (requestId === detailRequest.current) {
          detailReloadToken.current = requestReloadToken;
          setSelection({
            status: "ready",
            detail,
            entries: detail.entryPage.entries,
            nextCursor: detail.entryPage.nextCursor,
            loadingMore: false,
            refreshing: false,
            refreshError: false,
            pageError: false,
          });
        }
      } catch {
        if (requestId === detailRequest.current) {
          setSelection({status: "error", summary});
        }
      }
    },
    [client, reloadToken],
  );

  const loadMoreSessions = useCallback(async () => {
    if (library.nextCursor === null || library.loadingMore) return;
    const requestId = ++listRequest.current;
    const cursor = library.nextCursor;
    setLibrary((current) => ({...current, loadingMore: true}));
    try {
      const page = await client.listSessions({
        schemaVersion: 1,
        source: selectedSource === "all" ? null : selectedSource,
        query: query.trim(),
        sortOrder,
        cursor,
        pageSize: PAGE_SIZE,
      });
      if (requestId === listRequest.current) {
        setLibrary((current) => ({
          entries: mergeSessions(current.entries, page.items),
          nextCursor: page.nextCursor,
          status: "ready",
          loadingMore: false,
        }));
      }
    } catch {
      if (requestId === listRequest.current) {
        setLibrary((current) => ({...current, status: "error", loadingMore: false}));
      }
    }
  }, [client, library.loadingMore, library.nextCursor, query, selectedSource, sortOrder]);

  const changeSortOrder = useCallback((nextSortOrder: IndexedSessionSortOrderV1) => {
    ++listRequest.current;
    setLibrary(INITIAL_LIBRARY_STATE);
    setSortOrder(nextSortOrder);
  }, []);

  const changeSelectedSource = useCallback((nextSource: SessionSourceFilter) => {
    const current = selectionRef.current;
    const currentSource = current.status === "ready"
      ? current.detail.summary.source
      : current.status === "idle"
        ? null
        : current.summary.source;
    if (nextSource !== "all" && currentSource !== null && currentSource !== nextSource) {
      ++detailRequest.current;
      ++presentationRequest.current;
      setSelection({status: "idle"});
      setPresenting(false);
      setPresentationPlan(null);
      setPresentationError(false);
    }
    setSelectedSource(nextSource);
  }, []);

  const loadMoreEntries = useCallback(async () => {
    if (
      selection.status !== "ready" ||
      selection.nextCursor === null ||
      selection.loadingMore ||
      selection.refreshing
    ) {
      return;
    }
    const requestId = ++detailRequest.current;
    const current = selection;
    setSelection({...current, loadingMore: true, pageError: false});
    try {
      const next = await client.getSession(current.detail.summary.sessionId, current.nextCursor);
      if (requestId !== detailRequest.current) return;
      if (next.revision.revisionId !== current.detail.revision.revisionId) {
        const refreshed = await client.getSession(current.detail.summary.sessionId, null);
        if (requestId !== detailRequest.current) return;
        setSelection({
          status: "ready",
          detail: refreshed,
          entries: refreshed.entryPage.entries,
          nextCursor: refreshed.entryPage.nextCursor,
          loadingMore: false,
          refreshing: false,
          refreshError: false,
          pageError: false,
        });
        return;
      }
      setSelection({
        ...current,
        entries: mergeEntries(current.entries, next.entryPage.entries),
        nextCursor: next.entryPage.nextCursor,
        loadingMore: false,
        pageError: false,
      });
    } catch {
      if (requestId === detailRequest.current) {
        setSelection({...current, loadingMore: false, pageError: true});
      }
    }
  }, [client, selection]);

  const refresh = useCallback(async () => {
    setRefreshRequestPending(true);
    setRefreshCommandFailed(false);
    setResetCompleted(false);
    try {
      acceptRefreshState(await client.refresh());
    } catch {
      setRefreshCommandFailed(true);
    } finally {
      setRefreshRequestPending(false);
    }
  }, [acceptRefreshState, client]);

  const reloadAfterStaleCuration = useCallback(async (sessionId: string) => {
    const requestId = ++detailRequest.current;
    ++presentationRequest.current;
    setPresenting(false);
    setPresentationPlan(null);
    setPresentationError(false);
    try {
      const detail = await client.getSession(sessionId, null);
      if (requestId !== detailRequest.current) return false;
      const latest = selectionRef.current;
      if (latest.status !== "ready" || latest.detail.summary.sessionId !== sessionId) return false;
      setSelection({
        status: "ready",
        detail,
        entries: detail.entryPage.entries,
        nextCursor: detail.entryPage.nextCursor,
        loadingMore: false,
        refreshing: false,
        refreshError: false,
        pageError: false,
      });
      return true;
    } catch {
      return false;
    }
  }, [client]);

  const persistEntrySelections = useCallback(async (
    changes: readonly {readonly entryKey: string; readonly selected: boolean}[],
  ) => {
    const current = selectionRef.current;
    if (current.status !== "ready" || changes.length === 0) return;
    const previous = current;
    const byKey = new Map(changes.map((change) => [change.entryKey, change.selected]));
    const updatedEntries = current.entries.map((item) => ({
      ...item,
      selected: byKey.get(item.entry.entryKey) ?? item.selected,
    }));
    setSelection({...current, entries: updatedEntries});
    try {
      const detail = await client.setEntrySelections({
        schemaVersion: 1,
        sessionId: current.detail.summary.sessionId,
        revisionId: current.detail.revision.revisionId,
        changes,
      });
      setSelection((latest) =>
        latest.status === "ready" &&
        latest.detail.summary.sessionId === current.detail.summary.sessionId &&
        latest.detail.revision.revisionId === current.detail.revision.revisionId
          ? {...latest, detail, entries: updatedEntries}
          : latest,
      );
    } catch (error) {
      const reloaded = isErrorCode(error, "STALE_REVISION") &&
        await reloadAfterStaleCuration(current.detail.summary.sessionId);
      if (!reloaded) {
        setSelection((latest) =>
          latest.status === "ready" &&
          latest.detail.revision.revisionId === current.detail.revision.revisionId
            ? previous
            : latest,
        );
      }
      throw error;
    }
  }, [client, reloadAfterStaleCuration]);

  const persistPreferences = useCallback(async (
    preferences: SessionPreferencesV1,
  ) => {
    const current = selectionRef.current;
    if (current.status !== "ready") return;
    const previous = current;
    setSelection({...current, detail: {...current.detail, preferences}});
    try {
      const detail = await client.setSessionPreferences({
        schemaVersion: 1,
        sessionId: current.detail.summary.sessionId,
        revisionId: current.detail.revision.revisionId,
        preferences,
      });
      setSelection((latest) =>
        latest.status === "ready" &&
        latest.detail.revision.revisionId === current.detail.revision.revisionId
          ? {...latest, detail}
          : latest,
      );
    } catch (error) {
      const reloaded = isErrorCode(error, "STALE_REVISION") &&
        await reloadAfterStaleCuration(current.detail.summary.sessionId);
      if (!reloaded) {
        setSelection((latest) =>
          latest.status === "ready" &&
          latest.detail.revision.revisionId === current.detail.revision.revisionId
            ? previous
            : latest,
        );
      }
      throw error;
    }
  }, [client, reloadAfterStaleCuration]);

  const persistSessionTitle = useCallback(async (title: string) => {
    const current = selectionRef.current;
    if (current.status !== "ready") return;
    const previousTitle = current.detail.summary.title;
    const applyTitle = (nextTitle: string) => {
      setLibrary((libraryState) => ({
        ...libraryState,
        entries: libraryState.entries.map((entry) =>
          entry.sessionId === current.detail.summary.sessionId
            ? {...entry, title: nextTitle}
            : entry,
        ),
      }));
      setSelection((latest) =>
        latest.status === "ready" &&
        latest.detail.summary.sessionId === current.detail.summary.sessionId
          ? {
              ...latest,
              detail: {
                ...latest.detail,
                summary: {...latest.detail.summary, title: nextTitle},
              },
            }
          : latest,
      );
    };
    applyTitle(title);
    try {
      const detail = await client.renameSession({
        schemaVersion: 1,
        sessionId: current.detail.summary.sessionId,
        title,
      });
      applyTitle(detail.summary.title);
    } catch (error) {
      applyTitle(previousTitle);
      throw error;
    }
  }, [client]);

  const isRefreshing = refreshRequestPending || refreshState.status === "running";
  const resetLibraryView = useCallback(() => {
    ++listRequest.current;
    ++detailRequest.current;
    ++presentationRequest.current;
    lastCompletedGeneration.current = 0;
    detailReloadToken.current = 0;
    latestRefreshState.current = INITIAL_REFRESH_STATE;
    setLibrary({...INITIAL_LIBRARY_STATE, status: "ready"});
    setSelection({status: "idle"});
    setRefreshState(INITIAL_REFRESH_STATE);
    setRefreshCommandFailed(false);
    setResetCompleted(true);
    setStartupReady(true);
    setShowSuppressed(false);
    setShowSettings(false);
    setPresentationPlan(null);
    setPresentationError(false);
  }, []);
  const present = useCallback(async () => {
    const current = selectionRef.current;
    if (current.status !== "ready") return;
    const requestId = ++presentationRequest.current;
    const sessionId = current.detail.summary.sessionId;
    const revisionId = current.detail.revision.revisionId;
    setPresenting(true);
    setPresentationError(false);
    try {
      const plan = await client.createPresentationPlan(sessionId, revisionId);
      const latest = selectionRef.current;
      if (
        requestId === presentationRequest.current &&
        latest.status === "ready" &&
        latest.detail.summary.sessionId === sessionId &&
        latest.detail.revision.revisionId === revisionId
      ) {
        setPresentationPlan(plan);
      }
    } catch {
      if (requestId === presentationRequest.current) setPresentationError(true);
    } finally {
      if (requestId === presentationRequest.current) setPresenting(false);
    }
  }, [client]);
  const exportCurrentSession = useCallback(async () => {
    const current = selectionRef.current;
    if (current.status !== "ready") return;
    const requestId = ++exportRequest.current;
    const sessionId = current.detail.summary.sessionId;
    const revisionId = current.detail.revision.revisionId;
    let unsubscribe: () => void = () => {};
    let jobId: string | null = null;
    completedExportJob.current = null;
    setSessionExport({status: "preparing", progressPercent: null, message: "Preparing export…"});
    try {
      const plan = await client.createPresentationPlan(sessionId, revisionId);
      if (requestId !== exportRequest.current) return;
      const outputPath = await save({
        title: "Export presentation",
        defaultPath: defaultExportFileName(),
        filters: [{name: "MP4 video", extensions: ["mp4"]}],
      });
      if (requestId !== exportRequest.current) return;
      if (!outputPath) {
        setSessionExport(INITIAL_EXPORT_STATE);
        return;
      }
      jobId = `export_${Date.now().toString(36)}`;
      activeExportJob.current = jobId;
      setSessionExport({status: "exporting", progressPercent: null, message: "Preparing renderer…"});
      unsubscribe = await client.subscribeToExportProgress(jobId, (progress) => {
        if (activeExportJob.current !== progress.jobId) return;
        setSessionExport({
          status: "exporting",
          progressPercent: Math.floor((progress.renderedFrames / progress.totalFrames) * 100),
          message: "Export in progress…",
        });
      });
      await client.exportPresentation(plan.planId, outputPath, jobId);
      if (requestId === exportRequest.current) {
        completedExportJob.current = jobId;
        setSessionExport({status: "succeeded", progressPercent: 100, message: "Export complete"});
      }
    } catch (error) {
      if (requestId === exportRequest.current) {
        setSessionExport({
          status: exportWasCancelled(error) ? "cancelled" : "failed",
          progressPercent: null,
          message: exportFailureMessage(error),
        });
      }
    } finally {
      unsubscribe();
      if (activeExportJob.current === jobId) activeExportJob.current = null;
    }
  }, [client]);
  const cancelCurrentExport = useCallback(async () => {
    const jobId = activeExportJob.current;
    if (!jobId) return;
    try {
      await client.cancelExport(jobId);
    } catch {
      setSessionExport((current) => ({...current, message: "Cancellation unavailable; export continues"}));
    }
  }, [client]);
  const revealCurrentExport = useCallback(async () => {
    const jobId = completedExportJob.current;
    if (!jobId) return;
    try {
      await client.revealExport(jobId);
    } catch {
      setSessionExport((current) => ({...current, message: "Export complete; folder unavailable"}));
    }
  }, [client]);
  const selectedId =
    selection.status === "idle"
      ? null
      : selection.status === "ready"
        ? selection.detail.summary.sessionId
        : selection.summary.sessionId;
  if (!startupReady && library.entries.length === 0) {
    return (
      <div className="session-browser session-browser--initializing">
        <section className="app-loading-screen" role="status" aria-label="Loading AI Session Replay" aria-live="polite" aria-busy="true">
          <LoaderCircle className="spin" aria-hidden="true" size={30} />
          <strong>Preparing your local library</strong>
          <span>Discovering and indexing local AI sessions.</span>
        </section>
      </div>
    );
  }

  return (
    <div className={`session-browser${sourcesCollapsed ? " session-browser--sources-collapsed" : ""}`}>
      <SourceSidebar
        entries={library.entries}
        selectedSource={selectedSource}
        collapsed={sourcesCollapsed}
        jetBrainsStatus={jetBrainsStatus}
        onSelectSource={changeSelectedSource}
        onToggleCollapsed={() => setSourcesCollapsed((collapsed) => !collapsed)}
      />
      <div className="session-browser__content">
        <header className="browser-toolbar">
          <div>
            <p className="eyebrow">Local library</p>
            <h2>Indexed sessions</h2>
            <p className="browser-toolbar__status" role={refreshStatusIsError(refreshState, refreshCommandFailed) ? "alert" : "status"}>
              {refreshStatus(refreshState, refreshCommandFailed, resetCompleted)}
            </p>
          </div>
          <div className="browser-toolbar__actions">
            <button
              className="toolbar-button"
              type="button"
              aria-label="Open library settings"
              onClick={() => setShowSettings(true)}
            >
              <Settings aria-hidden="true" size={16} />Settings
            </button>
            <button
              className="toolbar-button"
              type="button"
              aria-label="Manage hidden sessions"
              onClick={() => setShowSuppressed(true)}
            >
              <ArchiveRestore aria-hidden="true" size={16} />Hidden
            </button>
            <button
              className="refresh-button"
              type="button"
              aria-label={isRefreshing ? "Refreshing sessions" : "Refresh"}
              disabled={isRefreshing}
              onClick={() => void refresh()}
            >
              <RefreshCw className={isRefreshing ? "spin" : undefined} aria-hidden="true" size={16} />
              {isRefreshing ? "Refreshing…" : "Refresh"}
            </button>
          </div>
        </header>
        <div className="session-browser__panes">
          <SessionList
            entries={library.entries}
            query={query}
            sortOrder={sortOrder}
            selectedId={selectedId}
            loadingId={selection.status === "loading" ? selection.summary.sessionId : null}
            loading={library.status === "loading"}
            error={library.status === "error"}
            hasMore={library.nextCursor !== null}
            loadingMore={library.loadingMore}
            onQueryChange={setQuery}
            onSortOrderChange={changeSortOrder}
            onSelect={(summary) => void selectSession(summary)}
            onLoadMore={() => void loadMoreSessions()}
            onRetry={() => void loadFirstPage()}
          />
          <SessionSelection
            key={selection.status === "ready" ? selection.detail.revision.revisionId : selection.status}
            selection={selection}
            onRetry={(summary) => void selectSession(summary)}
            onLoadMore={() => void loadMoreEntries()}
            onDelete={setDeletionTarget}
            onPresent={present}
            presenting={presenting}
            presentationError={presentationError}
            exportState={sessionExport}
            onExport={() => void exportCurrentSession()}
            onCancelExport={() => void cancelCurrentExport()}
            onRevealExport={() => void revealCurrentExport()}
            onEntrySelection={(entryKey, selected) =>
              persistEntrySelections([{entryKey, selected}])}
            onBulkSelection={(selected) => {
              const current = selectionRef.current;
              return current.status === "ready"
                ? persistEntrySelections(
                    current.entries
                      .filter((entry) => entry.selected !== selected)
                      .slice(0, 500)
                      .map((entry) => ({entryKey: entry.entry.entryKey, selected})),
                  )
                : Promise.resolve();
            }}
            onRename={persistSessionTitle}
            onPreferencesChange={persistPreferences}
          />
        </div>
      </div>
      {deletionTarget ? (
        <SessionDeletionDialog
          client={client}
          session={deletionTarget}
          onClose={() => setDeletionTarget(null)}
          onDeleted={() => {
            ++listRequest.current;
            ++detailRequest.current;
            ++presentationRequest.current;
            setLibrary((current) => ({
              ...current,
              entries: current.entries.filter(
                ({sessionId}) => sessionId !== deletionTarget.sessionId,
              ),
            }));
            setSelection({status: "idle"});
            setDeletionTarget(null);
          }}
        />
      ) : null}
      {showSuppressed ? (
        <SuppressedSourcesDialog client={client} onClose={() => setShowSuppressed(false)} />
      ) : null}
      {showSettings ? (
        <LibrarySettingsDialog
          client={client}
          refreshRunning={isRefreshing}
          onClose={() => setShowSettings(false)}
          onReset={resetLibraryView}
        />
      ) : null}
      {presentationPlan ? (
        <PresentationDialog client={client} plan={presentationPlan} onClose={() => setPresentationPlan(null)} />
      ) : null}
    </div>
  );
}

function mergeSessions(
  existing: readonly IndexedSessionSummaryV1[],
  incoming: readonly IndexedSessionSummaryV1[],
): readonly IndexedSessionSummaryV1[] {
  const byId = new Map(existing.map((session) => [session.sessionId, session]));
  for (const session of incoming) byId.set(session.sessionId, session);
  return [...byId.values()];
}

function mergeEntries<
  T extends {readonly entry: {readonly entryKey: string; readonly ordinal: number}},
>(
  existing: readonly T[],
  incoming: readonly T[],
): readonly T[] {
  const byKey = new Map(existing.map((item) => [item.entry.entryKey, item]));
  for (const item of incoming) byKey.set(item.entry.entryKey, item);
  return [...byKey.values()].sort((left, right) => left.entry.ordinal - right.entry.ordinal);
}

function refreshStatus(
  state: IndexRefreshStateV1,
  commandFailed: boolean,
  resetCompleted: boolean,
): string {
  if (commandFailed) return "Refresh status is unavailable. Showing the indexed library.";
  if (resetCompleted) return "Local library reset. Refresh to index source sessions again.";
  switch (state.status) {
    case "idle":
      return "Browse saved conversations while source refresh runs in the background.";
    case "running":
      return `Refreshing ${state.processedCount} of ${state.discoveredCount} sessions…${state.warningCount === 0 ? "" : ` ${state.warningCount} source warning${state.warningCount === 1 ? "" : "s"}.`}`;
    case "completed": {
      const outcomes = [
        `${state.indexedCount} updated`,
        `${state.unchangedCount} unchanged`,
        ...(state.failedCount === 0 ? [] : [`${state.failedCount} failed`]),
        ...(state.skippedCount === 0 ? [] : [`${state.skippedCount} suppressed`]),
        ...(state.warningCount === 0 ? [] : [`${state.warningCount} source warning${state.warningCount === 1 ? "" : "s"}`]),
      ];
      return `Library refreshed. ${outcomes.join(", ")}.`;
    }
    case "failed":
      return "Refresh failed. Showing last-known-good indexed sessions.";
  }
}

function refreshStatusIsError(state: IndexRefreshStateV1, commandFailed: boolean): boolean {
  return commandFailed || state.status === "failed";
}

function refreshStateAdvances(
  current: IndexRefreshStateV1,
  next: IndexRefreshStateV1,
): boolean {
  const currentGeneration = current.status === "idle" ? 0 : current.generation;
  const nextGeneration = next.status === "idle" ? 0 : next.generation;
  if (nextGeneration !== currentGeneration) return nextGeneration > currentGeneration;
  if (current.status === "idle") return false;
  if (current.status !== "running" || next.status === "idle") return false;
  return next.status !== "running" || next.processedCount >= current.processedCount;
}

function isErrorCode(error: unknown, code: string): boolean {
  return typeof error === "object" && error !== null && "code" in error && error.code === code;
}
