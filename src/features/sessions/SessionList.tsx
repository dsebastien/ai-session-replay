import {Archive, Clock3, LoaderCircle, Search} from "lucide-react";
import type {
  IndexedSessionSortOrderV1,
  IndexedSessionSummaryV1,
} from "../../../packages/replay-contract/src";
import {sourceLabel} from "./SourceSidebar";

interface SessionListProps {
  readonly entries: readonly IndexedSessionSummaryV1[];
  readonly query: string;
  readonly sortOrder: IndexedSessionSortOrderV1;
  readonly selectedId: string | null;
  readonly loadingId: string | null;
  readonly loading: boolean;
  readonly error: boolean;
  readonly hasMore: boolean;
  readonly loadingMore: boolean;
  readonly onQueryChange: (query: string) => void;
  readonly onSortOrderChange: (sortOrder: IndexedSessionSortOrderV1) => void;
  readonly onSelect: (entry: IndexedSessionSummaryV1) => void;
  readonly onLoadMore: () => void;
  readonly onRetry: () => void;
}

export function SessionList({
  entries,
  query,
  sortOrder,
  selectedId,
  loadingId,
  loading,
  error,
  hasMore,
  loadingMore,
  onQueryChange,
  onSortOrderChange,
  onSelect,
  onLoadMore,
  onRetry,
}: SessionListProps) {
  const hasQuery = query.trim().length > 0;
  return (
    <section className="session-list" aria-labelledby="session-list-title" aria-busy={loading}>
      <div className="session-list__heading">
        <div>
          <p className="eyebrow">Library</p>
          <h2 id="session-list-title">Sessions</h2>
        </div>
        <span className="result-count" aria-live="polite">
          {loading && entries.length === 0 ? "Loading…" : `${entries.length} shown`}
        </span>
      </div>

      <div className="session-list__filters">
        <label className="search-field">
          <span className="sr-only">Search sessions</span>
          <Search aria-hidden="true" size={16} />
          <input
            type="search"
            value={query}
            placeholder="Search titles, conversations, or dates"
            aria-label="Search sessions"
            maxLength={256}
            onChange={(event) => onQueryChange(event.currentTarget.value)}
          />
        </label>
        <label className="sort-field">
          <span>Sort</span>
          <select
            aria-label="Sort sessions"
            value={sortOrder}
            onChange={(event) => onSortOrderChange(event.currentTarget.value as IndexedSessionSortOrderV1)}
          >
            <option value="newest">Newest first</option>
            <option value="oldest">Oldest first</option>
          </select>
        </label>
      </div>

      <div className="session-list__notice" aria-live="polite">
        {loading ? (
          <span><LoaderCircle className="spin" aria-hidden="true" size={14} />Loading indexed library…</span>
        ) : error ? (
          <span role="alert">Indexed library could not be loaded.</span>
        ) : null}
      </div>

      {entries.length === 0 && !loading ? (
        <div className="list-empty" role={error ? undefined : "status"}>
          <strong>
            {error
              ? "Library unavailable"
              : hasQuery
                ? "No matching sessions"
                : "No indexed sessions yet"}
          </strong>
          <span>
            {error
              ? "The existing library could not be read."
              : hasQuery
                ? "Try different words or clear the search."
              : "Refresh to index supported local session sources."}
          </span>
          {error ? (
            <button type="button" onClick={onRetry}>Try again</button>
          ) : null}
        </div>
      ) : (
        <>
          <ul className="session-items">
            {entries.map((entry) => {
              const isSelected = entry.sessionId === selectedId;
              const isLoading = entry.sessionId === loadingId;
              return (
                <li key={entry.sessionId}>
                  <button
                    type="button"
                    className="session-row"
                    aria-current={isSelected ? "true" : undefined}
                    aria-label={`Open ${entry.title} from ${sourceLabel(entry.source)}`}
                    disabled={isLoading}
                    onClick={() => onSelect(entry)}
                  >
                    <span className="session-row__topline">
                      <strong>{entry.title}</strong>
                      <span className="session-row__source">
                        {isLoading ? "Opening…" : sourceLabel(entry.source)}
                      </span>
                    </span>
                    <span className="session-row__meta">
                      <span>{pluralize(entry.entryCount, "entry")}</span>
                      <span>{formatDuration(entry.durationMs)}</span>
                      <span><Clock3 aria-hidden="true" size={13} />{formatSessionDate(entry.createdAtMs ?? entry.lastIndexedAtMs)}</span>
                    </span>
                    {!entry.sourcePresent ? (
                      <span className="session-row__saved"><Archive aria-hidden="true" size={13} />Indexed copy</span>
                    ) : null}
                  </button>
                </li>
              );
            })}
          </ul>
          {hasMore ? (
            <button
              className="load-more-button"
              type="button"
              disabled={loading || loadingMore}
              onClick={onLoadMore}
            >
              {loadingMore ? "Loading…" : "Load more sessions"}
            </button>
          ) : null}
        </>
      )}
    </section>
  );
}

function pluralize(count: number, noun: string): string {
  return `${count} ${count === 1 ? noun : noun === "entry" ? "entries" : `${noun}s`}`;
}

function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.max(0, Math.round(milliseconds / 1_000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes === 0 ? `${seconds}s` : `${minutes}m ${seconds.toString().padStart(2, "0")}s`;
}

function formatSessionDate(timestamp: number): string {
  const date = new Date(timestamp);
  return Number.isNaN(date.getTime())
    ? "Date unavailable"
    : new Intl.DateTimeFormat(undefined, {dateStyle: "medium"}).format(date);
}
