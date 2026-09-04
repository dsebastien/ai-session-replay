import {ArchiveRestore, LoaderCircle} from "lucide-react";
import {useEffect, useRef, useState} from "react";
import type {SuppressedSourceV1} from "../../../packages/replay-contract/src";
import type {IndexedSessionClient} from "../../lib/tauri/indexed-sessions";
import {sourceLabel} from "./SourceSidebar";
import {useModalDialog} from "./useModalDialog";

interface SuppressedSourcesDialogProps {
  readonly client: IndexedSessionClient;
  readonly onClose: () => void;
}

const PAGE_SIZE = 40;

export function SuppressedSourcesDialog({client, onClose}: SuppressedSourcesDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const doneButtonRef = useRef<HTMLButtonElement>(null);
  const [items, setItems] = useState<readonly SuppressedSourceV1[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  const [loadingMore, setLoadingMore] = useState(false);
  const [restoringId, setRestoringId] = useState<string | null>(null);
  const [restoreMessage, setRestoreMessage] = useState<string | null>(null);
  useModalDialog(dialogRef, doneButtonRef);

  useEffect(() => {
    let active = true;
    void client
      .listSuppressed({schemaVersion: 1, cursor: null, pageSize: PAGE_SIZE})
      .then((page) => {
        if (!active) return;
        setItems(page.items);
        setNextCursor(page.nextCursor);
        setStatus("ready");
      })
      .catch(() => {
        if (active) setStatus("error");
      });
    return () => {
      active = false;
    };
  }, [client]);

  const loadMore = async () => {
    if (!nextCursor || loadingMore) return;
    setLoadingMore(true);
    try {
      const page = await client.listSuppressed({
        schemaVersion: 1,
        cursor: nextCursor,
        pageSize: PAGE_SIZE,
      });
      setItems((current) => mergeSuppressed(current, page.items));
      setNextCursor(page.nextCursor);
    } catch {
      setStatus("error");
    } finally {
      setLoadingMore(false);
    }
  };

  const restore = async (item: SuppressedSourceV1) => {
    setRestoringId(item.suppressionId);
    setRestoreMessage(null);
    try {
      await client.restoreSuppressed(item.suppressionId);
      setItems((current) => current.filter(({suppressionId}) => suppressionId !== item.suppressionId));
      setRestoreMessage("Source restored. Refresh to index it again.");
    } catch {
      setRestoreMessage("The hidden source could not be restored.");
    } finally {
      setRestoringId(null);
    }
  };

  const busy = restoringId !== null;
  return (
    <dialog
      ref={dialogRef}
      className="modal-dialog suppressed-dialog"
      aria-labelledby="suppressed-dialog-title"
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <div className="modal-dialog__icon"><ArchiveRestore aria-hidden="true" size={20} /></div>
      <p className="eyebrow">Library suppression</p>
      <h2 id="suppressed-dialog-title">Hidden sessions</h2>
      <p>Restore a source to let a future refresh index it again. No source files are recreated.</p>
      <div className="suppressed-dialog__status" aria-live="polite">
        {status === "loading" ? (
          <span><LoaderCircle className="spin" aria-hidden="true" size={14} />Loading hidden sessions…</span>
        ) : status === "error" ? (
          <span role="alert">Hidden sessions could not be loaded.</span>
        ) : restoreMessage ? (
          <span>{restoreMessage}</span>
        ) : null}
      </div>
      {status === "ready" && items.length === 0 ? (
        <div className="suppressed-dialog__empty">No hidden sessions.</div>
      ) : (
        <ul className="suppressed-list">
          {items.map((item) => (
            <li key={item.suppressionId}>
              <div>
                <strong>{sourceLabel(item.source)}</strong>
                <span>{item.sourceDeleted ? "Source deleted" : "Source retained"}</span>
                <small>Hidden {formatSuppressedAt(item.suppressedAtMs)}</small>
              </div>
              <button
                type="button"
                className="button-secondary"
                aria-label={`Restore ${sourceLabel(item.source)} source`}
                disabled={busy}
                onClick={() => void restore(item)}
              >
                {restoringId === item.suppressionId ? "Restoring…" : "Restore"}
              </button>
            </li>
          ))}
        </ul>
      )}
      {nextCursor ? (
        <button type="button" className="load-more-button" disabled={loadingMore || busy} onClick={() => void loadMore()}>
          {loadingMore ? "Loading…" : "Load more hidden sessions"}
        </button>
      ) : null}
      <div className="modal-dialog__actions">
        <button ref={doneButtonRef} type="button" className="button-primary" disabled={busy} onClick={onClose}>Done</button>
      </div>
    </dialog>
  );
}

function mergeSuppressed(
  current: readonly SuppressedSourceV1[],
  incoming: readonly SuppressedSourceV1[],
): readonly SuppressedSourceV1[] {
  const byId = new Map(current.map((item) => [item.suppressionId, item]));
  for (const item of incoming) byId.set(item.suppressionId, item);
  return [...byId.values()];
}

function formatSuppressedAt(timestamp: number): string {
  const date = new Date(timestamp);
  return Number.isNaN(date.getTime())
    ? "at an unknown date"
    : new Intl.DateTimeFormat(undefined, {dateStyle: "medium"}).format(date);
}
