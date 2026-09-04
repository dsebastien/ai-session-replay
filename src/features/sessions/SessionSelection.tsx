import {
  Archive,
  Bot,
  CircleCheck,
  CircleAlert,
  FileClock,
  LoaderCircle,
  Pencil,
} from "lucide-react";
import {useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent} from "react";
import {
  ConversationCard,
  entryPresentation,
} from "../../../packages/remotion-composition/src/ConversationCard";
import type {
  IndexedSessionDetailV1,
  IndexedSessionSummaryV1,
  SelectableEntryV1,
  SessionPreferencesV1,
} from "../../../packages/replay-contract/src";
import {sourceLabel} from "./SourceSidebar";
import {ToastNotification} from "./ToastNotification";

const MAX_RENDERED_ENTRIES = 100;
const EMPTY_ENTRIES: readonly SelectableEntryV1[] = [];

export type SessionSelectionState =
  | Readonly<{status: "idle"}>
  | Readonly<{status: "loading"; summary: IndexedSessionSummaryV1}>
  | Readonly<{
      status: "ready";
      detail: IndexedSessionDetailV1;
      entries: readonly SelectableEntryV1[];
      nextCursor: string | null;
      loadingMore: boolean;
      refreshing: boolean;
      refreshError: boolean;
      pageError: boolean;
    }>
  | Readonly<{status: "error"; summary: IndexedSessionSummaryV1}>;

export type SessionExportState = Readonly<{
  status: "idle" | "preparing" | "exporting" | "succeeded" | "failed" | "cancelled";
  progressPercent: number | null;
  message: string | null;
}>;

interface SessionSelectionProps {
  readonly selection: SessionSelectionState;
  readonly onRetry: (summary: IndexedSessionSummaryV1) => void;
  readonly onLoadMore: () => void;
  readonly onDelete: (summary: IndexedSessionSummaryV1) => void;
  readonly onPresent: () => void;
  readonly presenting: boolean;
  readonly presentationError: boolean;
  readonly exportState: SessionExportState;
  readonly onExport: () => void;
  readonly onCancelExport: () => void;
  readonly onRevealExport: () => void;
  readonly onEntrySelection: (entryKey: string, selected: boolean) => Promise<void>;
  readonly onBulkSelection: (selected: boolean) => Promise<void>;
  readonly onRename: (title: string) => Promise<void>;
  readonly onPreferencesChange: (preferences: SessionPreferencesV1) => Promise<void>;
}

export function SessionSelection({
  selection,
  onRetry,
  onLoadMore,
  onDelete,
  onPresent,
  presenting,
  presentationError,
  exportState,
  onExport,
  onCancelExport,
  onRevealExport,
  onEntrySelection,
  onBulkSelection,
  onRename,
  onPreferencesChange,
}: SessionSelectionProps) {
  const [curating, setCurating] = useState(false);
  const [curationError, setCurationError] = useState(false);
  const [curationConfirmation, setCurationConfirmation] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [renameSaving, setRenameSaving] = useState(false);
  const [renameError, setRenameError] = useState(false);
  const [renameConfirmation, setRenameConfirmation] = useState(false);
  const [draftTitle, setDraftTitle] = useState("");
  const [entryWindowStart, setEntryWindowStart] = useState(0);
  const [activeEntryKey, setActiveEntryKey] = useState<string | null>(null);
  const [savingEntryKeys, setSavingEntryKeys] = useState<ReadonlySet<string>>(() => new Set());
  const entriesRef = useRef<HTMLOListElement>(null);
  const readyEntries = selection.status === "ready" ? selection.entries : EMPTY_ENTRIES;
  const conversationIdentity = selection.status === "ready"
    ? `${selection.detail.summary.sessionId}:${selection.detail.revision.revisionId}`
    : null;
  const visibility = selection.status === "ready" ? selection.detail.preferences.visibility : null;
  const visibleEntries = useMemo(() => readyEntries.filter(({entry}) => {
    if (entry.kind === "reasoning") return visibility?.showReasoning ?? false;
    if (entry.kind === "tool-call") return visibility?.showToolCalls ?? false;
    return true;
  }), [readyEntries, visibility]);
  const curationDisabled = curating || presenting;
  useLayoutEffect(() => {
    if (conversationIdentity === null) return;
    setEntryWindowStart(0);
    setCurationConfirmation(null);
    setRenaming(false);
    setRenameSaving(false);
    setRenameError(false);
    setRenameConfirmation(false);
    setSavingEntryKeys(new Set());
    const focusEntry = window.setTimeout(() => {
      const firstEntry = entriesRef.current?.querySelector<HTMLElement>(".conversation-entry");
      setActiveEntryKey(firstEntry?.dataset.entryKey ?? null);
      firstEntry?.focus({preventScroll: true});
    }, 0);
    return () => window.clearTimeout(focusEntry);
  }, [conversationIdentity]);

  const curate = async (operation: () => Promise<void>, confirmation?: string) => {
    setCurating(true);
    setCurationError(false);
    setCurationConfirmation(null);
    try {
      await operation();
      setCurationConfirmation(confirmation ?? null);
    } catch {
      setCurationError(true);
    } finally {
      setCurating(false);
    }
  };

  const saveEntrySelection = async (entryKey: string, selected: boolean) => {
    setSavingEntryKeys((current) => new Set(current).add(entryKey));
    setCurationError(false);
    setCurationConfirmation(null);
    try {
      await onEntrySelection(entryKey, selected);
      setCurationConfirmation(selected ? "Entry included." : "Entry excluded.");
    } catch {
      setCurationError(true);
    } finally {
      setSavingEntryKeys((current) => {
        const next = new Set(current);
        next.delete(entryKey);
        return next;
      });
    }
  };

  if (selection.status === "idle") {
    return (
      <section className="selection-panel selection-panel--empty" aria-labelledby="selection-title">
        <FileClock aria-hidden="true" size={28} strokeWidth={1.5} />
        <h2 id="selection-title">Select a session</h2>
        <p>Choose an indexed conversation from the library.</p>
      </section>
    );
  }

  if (selection.status === "loading") {
    return (
      <section className="selection-panel selection-panel--empty" aria-live="polite" aria-busy="true">
        <LoaderCircle className="spin" aria-hidden="true" size={24} />
        <strong>Opening indexed conversation…</strong>
        <span>The app-owned copy is read without reopening the source.</span>
      </section>
    );
  }

  if (selection.status === "error") {
    return (
      <section className="selection-panel selection-panel--error" role="alert">
        <CircleAlert aria-hidden="true" size={24} />
        <strong>This indexed session could not be opened</strong>
        <span>The library remains available; no source content was exposed.</span>
        <button type="button" onClick={() => onRetry(selection.summary)}>Retry selection</button>
      </section>
    );
  }

  const {summary, revision, preferences} = selection.detail;
  const canPresent = summary.selectedEntryCount > 0;
  const windowStart = Math.min(
    entryWindowStart,
    Math.max(0, Math.floor(Math.max(0, visibleEntries.length - 1) / MAX_RENDERED_ENTRIES) * MAX_RENDERED_ENTRIES),
  );
  const renderedEntries = conversationWindow(visibleEntries, windowStart);
  const exportError = exportState.status === "failed" ||
    exportState.message === "Export complete; folder unavailable"
    ? exportState.message
    : null;
  const activeError = renameError
    ? "Session name could not be saved."
    : presentationError
      ? "Presentation could not be created. Reload the session and try again."
      : exportError
        ? exportError
        : curationError
          ? "Curation could not be saved; saved choices were restored or reloaded."
          : selection.refreshError
            ? "Conversation update unavailable. Showing the previous indexed revision."
            : selection.pageError
              ? "More messages could not be loaded."
              : null;
  const operationBusy = presenting || exportState.status === "preparing" || exportState.status === "exporting";
  const operationSucceeded = exportState.status === "succeeded" ||
    (exportState.status === "idle" && !presenting && renameConfirmation);
  const operationMessage = presenting
    ? "Preparing presentation…"
    : exportState.status === "preparing" || exportState.status === "exporting"
      ? exportState.message ?? "Export in progress…"
      : exportState.status === "succeeded"
        ? "Export complete"
          : exportState.status === "cancelled"
          ? "Export cancelled"
          : renameConfirmation
            ? "Session renamed."
            : "Ready to present or export";
  const curationStatus = savingEntryKeys.size > 0
    ? "Saving entry choice…"
    : curating
      ? "Saving curation…"
      : curationError
        ? null
        : curationConfirmation;
  const showEntryWindow = (nextStart: number) => {
    setEntryWindowStart(nextStart);
    window.setTimeout(() => {
      entriesRef.current?.scrollIntoView?.({block: "start"});
      entriesRef.current?.focus({preventScroll: true});
    }, 0);
  };
  const handleEntryKeyDown = (
    event: KeyboardEvent<HTMLLIElement>,
    index: number,
    selectable: SelectableEntryV1,
  ) => {
    const direction = event.key === "ArrowUp" || event.key === "ArrowLeft"
      ? -1
      : event.key === "ArrowDown" || event.key === "ArrowRight"
        ? 1
        : 0;
    if (direction !== 0) {
      event.preventDefault();
      const target = entriesRef.current?.querySelectorAll<HTMLElement>(".conversation-entry")
        .item(index + direction);
      target?.focus();
      target?.scrollIntoView?.({block: "nearest"});
      return;
    }
    if (
      event.target === event.currentTarget &&
      event.key === " " &&
      !curationDisabled &&
      !savingEntryKeys.has(selectable.entry.entryKey)
    ) {
      event.preventDefault();
      void saveEntrySelection(selectable.entry.entryKey, !selectable.selected);
    }
  };
  const submitRename = async () => {
    const title = draftTitle.trim();
    if (title.length === 0 || title.length > 512 || title === summary.title) return;
    setRenameSaving(true);
    setRenameError(false);
    setRenameConfirmation(false);
    try {
      await onRename(title);
      setRenaming(false);
      setRenameConfirmation(true);
    } catch {
      setRenameError(true);
    } finally {
      setRenameSaving(false);
    }
  };
  return (
    <section className="selection-panel conversation" aria-labelledby="selection-title" aria-busy={selection.refreshing}>
      {activeError ? (
        <ToastNotification key={activeError} message={activeError} />
      ) : null}
      <div className="conversation__chrome">
        <header className="conversation__header">
        <div>
          <p className="eyebrow">{sourceLabel(summary.source)}</p>
          <h2 id="selection-title">{summary.title}</h2>
        </div>
        <div className="conversation__actions">
          <span className={`source-presence${summary.sourcePresent ? "" : " source-presence--saved"}`}>
            {summary.sourcePresent ? <Bot aria-hidden="true" size={14} /> : <Archive aria-hidden="true" size={14} />}
            {summary.sourcePresent ? "Source available" : "Source unavailable — using indexed copy"}
          </span>
          <button
            type="button"
            className="button-secondary"
            aria-label="Rename session"
            disabled={renameSaving}
            onClick={() => {
              setDraftTitle(summary.title);
              setRenameError(false);
              setRenameConfirmation(false);
              setRenaming(true);
            }}
          >
            <Pencil aria-hidden="true" size={14} />Rename
          </button>
          <button type="button" className="conversation__delete" onClick={() => onDelete(summary)}>
            <Archive aria-hidden="true" size={14} />Hide session
          </button>
          <button
            type="button"
            className="button-primary"
            disabled={!canPresent || presenting || curating}
            title={!canPresent ? "Select at least one entry" : undefined}
            onClick={onPresent}
          >
            {presenting ? "Preparing…" : "Present"}
          </button>
          <button
            type="button"
            className="button-secondary"
            aria-label="Export session as MP4"
            disabled={!canPresent || exportState.status === "preparing" || exportState.status === "exporting"}
            onClick={onExport}
          >
            {exportState.status === "preparing"
              ? "Preparing export…"
              : exportState.status === "exporting"
                ? exportState.progressPercent === null
                  ? "Exporting…"
                  : `Exporting ${exportState.progressPercent}%`
                : "Export MP4"}
          </button>
        </div>
        </header>
        {renaming ? (
        <form
          className="conversation__rename"
          onSubmit={(event) => {
            event.preventDefault();
            void submitRename();
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape" && !renameSaving) {
              event.preventDefault();
              setRenaming(false);
              setRenameError(false);
            }
          }}
        >
          <label htmlFor="session-title-input">Session name</label>
          <input
            id="session-title-input"
            type="text"
            value={draftTitle}
            maxLength={512}
            autoFocus
            disabled={renameSaving}
            onChange={(event) => setDraftTitle(event.currentTarget.value)}
          />
          <button
            type="submit"
            className="button-primary"
            disabled={renameSaving || draftTitle.trim().length === 0 || draftTitle.trim() === summary.title}
          >
            {renameSaving ? "Saving…" : "Save name"}
          </button>
          <button type="button" className="button-secondary" disabled={renameSaving} onClick={() => setRenaming(false)}>
            Cancel
          </button>
        </form>
        ) : null}
        <div
          className={`conversation__operation-bar${operationBusy ? " conversation__operation-bar--busy" : ""}${operationSucceeded ? " conversation__operation-bar--success" : ""}`}
          role="status"
          aria-live="polite"
        >
          <div className="conversation__operation-copy">
            {operationBusy ? <LoaderCircle className="spin" aria-hidden="true" size={14} />
              : operationSucceeded ? <CircleCheck aria-hidden="true" size={14} />
                : <span className="conversation__operation-dot" aria-hidden="true" />}
            <span>{operationMessage}</span>
            {exportState.status === "exporting" && exportState.progressPercent !== null ? (
              <progress aria-label="Session export progress" max={100} value={exportState.progressPercent} />
            ) : null}
          </div>
          <div className="conversation__operation-actions">
            {exportState.status === "exporting" ? (
              <button type="button" className="button-secondary" onClick={onCancelExport}>Cancel export</button>
            ) : exportState.status === "succeeded" ? (
              <button type="button" className="button-secondary" onClick={onRevealExport}>Show in folder</button>
            ) : null}
          </div>
        </div>
        <dl className="conversation__summary">
        <div><dt>Entries</dt><dd>{summary.entryCount}</dd></div>
        <div><dt>Selected</dt><dd>{summary.selectedEntryCount}</dd></div>
        <div><dt>Duration</dt><dd>{formatDuration(summary.durationMs)}</dd></div>
        <div><dt>Revision</dt><dd>{formatIndexed(revision.indexedAtMs)}</dd></div>
        </dl>
        <div className="conversation__curation" aria-label="Conversation visibility and selection">
        <div>
          <span>Show</span>
          {([
            ["showToolCalls", "Tools"],
            ["showToolDetails", "Tool details"],
            ["showReasoning", "Reasoning"],
          ] as const).map(([field, label]) => (
            <label key={field}>
              <input
                type="checkbox"
                checked={preferences.visibility[field]}
                disabled={curationDisabled || (field === "showToolDetails" && !preferences.visibility.showToolCalls)}
                onChange={(event) => void curate(() => onPreferencesChange({
                  ...preferences,
                  visibility: {
                    ...preferences.visibility,
                    [field]: event.currentTarget.checked,
                    ...(field === "showToolCalls" && !event.currentTarget.checked
                      ? {showToolDetails: false}
                      : {}),
                  },
                }))}
              />
              {label}
            </label>
          ))}
        </div>
        <div>
          <button type="button" disabled={curationDisabled} onClick={() => void curate(() => onBulkSelection(true), "All loaded entries included.")}>Select loaded</button>
          <button type="button" disabled={curationDisabled} onClick={() => void curate(() => onBulkSelection(false), "All loaded entries excluded.")}>Exclude loaded</button>
        </div>
        <span className="conversation__keyboard-help">Use arrow keys to move; press Space to include or exclude.</span>
        <div className="conversation__presentation-settings">
          <label>Delay
            <select aria-label="Entry delay" disabled={curationDisabled} value={preferences.timing.entryDelayMs} onChange={(event) => void curate(() => onPreferencesChange({
              ...preferences,
              timing: {...preferences.timing, entryDelayMs: Number(event.currentTarget.value)},
            }))}>
              {[250, 500, 1_000, 1_500, 2_000, 3_000, 5_000, 10_000].map((delay) => (
                <option key={delay} value={delay}>{delay / 1_000}s</option>
              ))}
            </select>
          </label>
          <label>Speed
            <select aria-label="Playback speed" disabled={curationDisabled} value={preferences.timing.playbackSpeed} onChange={(event) => void curate(() => onPreferencesChange({
              ...preferences,
              timing: {...preferences.timing, playbackSpeed: Number(event.currentTarget.value)},
            }))}>
              {[0.25, 0.5, 1, 1.5, 2, 4].map((speed) => (
                <option key={speed} value={speed}>{speed}×</option>
              ))}
            </select>
          </label>
          <label>Palette
            <select aria-label="Presentation palette" disabled={curationDisabled} value={themeName(preferences)} onChange={(event) => void curate(() => onPreferencesChange({
              ...preferences,
              appearance: {...preferences.appearance, theme: THEMES[event.currentTarget.value as keyof typeof THEMES]},
            }))}>
              <option value="terminal">Terminal</option>
              <option value="midnight">Midnight</option>
              <option value="contrast">High contrast</option>
            </select>
          </label>
          <label>Text size
            <input aria-label="Presentation text size" type="number" min={12} max={96} disabled={curationDisabled} value={preferences.appearance.font.sizePx} onChange={(event) => void curate(() => onPreferencesChange({
              ...preferences,
              appearance: {...preferences.appearance, font: {...preferences.appearance.font, sizePx: Number(event.currentTarget.value)}},
            }))} />
          </label>
          <label>Line height
            <select aria-label="Presentation line height" disabled={curationDisabled} value={preferences.appearance.font.lineHeight} onChange={(event) => void curate(() => onPreferencesChange({
              ...preferences,
              appearance: {...preferences.appearance, font: {...preferences.appearance.font, lineHeight: Number(event.currentTarget.value)}},
            }))}>
              {[1, 1.25, 1.5, 1.75, 2, 2.5].map((lineHeight) => (
                <option key={lineHeight} value={lineHeight}>{lineHeight}</option>
              ))}
            </select>
          </label>
        </div>
        </div>
        <div className="conversation__curation-status" aria-live="polite">
          {curationStatus}
        </div>
        <div className="conversation__sync-status" aria-live="polite">
          {selection.refreshing ? (
            <span><LoaderCircle className="spin" aria-hidden="true" size={13} />Updating indexed conversation…</span>
          ) : null}
        </div>
      </div>
      <div className="conversation__scroll">
        <ol ref={entriesRef} tabIndex={-1} aria-describedby="conversation-window-status" className="conversation__entries" aria-label="Conversation entries" style={{
        fontFamily: preferences.appearance.font.family,
        fontSize: Math.max(12, Math.round(preferences.appearance.font.sizePx * 0.47)),
        lineHeight: preferences.appearance.font.lineHeight,
      }}>
        {renderedEntries.map((selectable, index) => (
          <ConversationEntry
            active={activeEntryKey === selectable.entry.entryKey}
            key={selectable.entry.entryKey}
            selectable={selectable}
            showToolDetails={preferences.visibility.showToolDetails}
            theme={preferences.appearance.theme}
            totalEntryCount={summary.entryCount}
            disabled={curationDisabled || savingEntryKeys.has(selectable.entry.entryKey)}
            saving={savingEntryKeys.has(selectable.entry.entryKey)}
            onFocus={() => setActiveEntryKey(selectable.entry.entryKey)}
            onKeyDown={(event) => handleEntryKeyDown(event, index, selectable)}
            onSelectionChange={(selected) => void saveEntrySelection(selectable.entry.entryKey, selected)}
          />
        ))}
        </ol>
        <nav className="conversation__window-navigation" aria-label="Conversation entry pages">
        <button type="button" disabled={windowStart === 0} onClick={() => showEntryWindow(Math.max(0, windowStart - MAX_RENDERED_ENTRIES))}>
          Previous entries
        </button>
        <span id="conversation-window-status" role="status" aria-live="polite">
          {visibleEntries.length === 0 ? "No visible entries" : `Showing ${windowStart + 1}–${windowStart + renderedEntries.length} of ${visibleEntries.length} loaded`}
        </span>
        <button type="button" disabled={windowStart + MAX_RENDERED_ENTRIES >= visibleEntries.length} onClick={() => showEntryWindow(windowStart + MAX_RENDERED_ENTRIES)}>
          Next entries
        </button>
        </nav>
        {selection.nextCursor ? (
          <button className="load-more-button" type="button" disabled={selection.loadingMore} onClick={onLoadMore}>
            {selection.loadingMore ? "Loading…" : "Load more messages"}
          </button>
        ) : null}
      </div>
    </section>
  );
}

function ConversationEntry({
  active,
  selectable,
  showToolDetails,
  theme,
  totalEntryCount,
  disabled,
  saving,
  onFocus,
  onKeyDown,
  onSelectionChange,
}: {
  readonly active: boolean;
  readonly selectable: SelectableEntryV1;
  readonly showToolDetails: boolean;
  readonly theme: SessionPreferencesV1["appearance"]["theme"];
  readonly totalEntryCount: number;
  readonly disabled: boolean;
  readonly saving: boolean;
  readonly onFocus: () => void;
  readonly onKeyDown: (event: KeyboardEvent<HTMLLIElement>) => void;
  readonly onSelectionChange: (selected: boolean) => void;
}) {
  const {entry} = selectable;
  const alignment = entry.kind === "user" ? "user" : entry.kind === "assistant" ? "assistant" : "supporting";
  const label = entryPresentation(entry).label;
  return (
    <li
      className={`conversation-entry conversation-entry--${alignment}`}
      aria-posinset={entry.ordinal + 1}
      aria-setsize={totalEntryCount}
      aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight Space"
      aria-current={active ? "true" : undefined}
      aria-busy={saving}
      data-active={String(active)}
      data-entry-key={entry.entryKey}
      tabIndex={0}
      onFocus={onFocus}
      onKeyDown={onKeyDown}
    >
      <ConversationCard
        entry={entry}
        active={active}
        theme={theme}
        mode="conversation"
        showToolDetails={showToolDetails}
        trailing={
          <>
            {!selectable.selected ? <span>Excluded</span> : null}
          <label className="conversation-entry__selection">
            <input
              type="checkbox"
              checked={selectable.selected}
              disabled={disabled}
              aria-label={`${selectable.selected ? "Exclude" : "Include"} ${label} entry`}
              onChange={(event) => onSelectionChange(event.currentTarget.checked)}
            />
          </label>
          </>
        }
      />
    </li>
  );
}

export function conversationWindow<T>(entries: readonly T[], start: number): readonly T[] {
  const safeStart = Number.isSafeInteger(start) && start >= 0 ? start : 0;
  return entries.slice(safeStart, safeStart + MAX_RENDERED_ENTRIES);
}

const THEMES = {
  terminal: {background: "#0d0f0e", surface: "#151816", text: "#f3f2ee", muted: "#9ba09c", accent: "#d6aa68", success: "#9dccb3", error: "#e59a91"},
  midnight: {background: "#080c16", surface: "#111827", text: "#f8fafc", muted: "#94a3b8", accent: "#60a5fa", success: "#6ee7b7", error: "#fca5a5"},
  contrast: {background: "#000000", surface: "#111111", text: "#ffffff", muted: "#cfcfcf", accent: "#ffd400", success: "#7cff9b", error: "#ff8a80"},
} as const;

function themeName(preferences: SessionPreferencesV1): keyof typeof THEMES {
  const background = preferences.appearance.theme.background.toLowerCase();
  if (background === THEMES.midnight.background) return "midnight";
  if (background === THEMES.contrast.background) return "contrast";
  return "terminal";
}

function formatDuration(milliseconds: number): string {
  const seconds = Math.round(milliseconds / 1_000);
  const minutes = Math.floor(seconds / 60);
  return minutes === 0 ? `${seconds}s` : `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
}

function formatIndexed(timestamp: number): string {
  const date = new Date(timestamp);
  return Number.isNaN(date.getTime())
    ? "Date unavailable"
    : new Intl.DateTimeFormat(undefined, {dateStyle: "medium", timeStyle: "short"}).format(date);
}
