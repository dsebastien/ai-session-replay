import {AlertTriangle, LoaderCircle, Trash2} from "lucide-react";
import {useEffect, useRef, useState} from "react";
import type {
  IndexedSessionSummaryV1,
  SourceDeletionConfirmationV1,
  SuppressedSourceV1,
} from "../../../packages/replay-contract/src";
import type {IndexedSessionClient} from "../../lib/tauri/indexed-sessions";
import {sourceLabel} from "./SourceSidebar";
import {useModalDialog} from "./useModalDialog";

interface SessionDeletionDialogProps {
  readonly client: IndexedSessionClient;
  readonly session: IndexedSessionSummaryV1;
  readonly onClose: () => void;
  readonly onDeleted: (tombstone: SuppressedSourceV1) => void;
}

type DeletionPhase = "choose" | "preparing" | "confirm-source" | "deleting";

export function SessionDeletionDialog({
  client,
  session,
  onClose,
  onDeleted,
}: SessionDeletionDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const safeActionRef = useRef<HTMLButtonElement>(null);
  const [deleteSource, setDeleteSource] = useState(false);
  const [phase, setPhase] = useState<DeletionPhase>("choose");
  const [confirmation, setConfirmation] = useState<SourceDeletionConfirmationV1 | null>(null);
  const [failed, setFailed] = useState(false);
  useModalDialog(dialogRef, safeActionRef);
  useEffect(() => {
    if (phase === "choose" || phase === "confirm-source") safeActionRef.current?.focus();
  }, [phase]);

  const busy = phase === "preparing" || phase === "deleting";
  const cancel = () => {
    if (!busy) onClose();
  };

  const deleteOrReview = async () => {
    setFailed(false);
    if (!deleteSource) {
      setPhase("deleting");
      try {
        onDeleted(await client.deleteSession(session.sessionId));
      } catch {
        setFailed(true);
        setPhase("choose");
      }
      return;
    }

    setPhase("preparing");
    try {
      const prepared = await client.prepareSourceDeletion(session.sessionId);
      setConfirmation(prepared);
      setPhase("confirm-source");
    } catch {
      setFailed(true);
      setPhase("choose");
    }
  };

  const confirmSourceDeletion = async () => {
    if (!confirmation) return;
    setFailed(false);
    setPhase("deleting");
    try {
      onDeleted(
        await client.deleteSessionWithSource(
          session.sessionId,
          confirmation.confirmationToken,
        ),
      );
    } catch {
      setConfirmation(null);
      setFailed(true);
      setPhase("choose");
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="modal-dialog deletion-dialog"
      aria-labelledby="deletion-dialog-title"
      onCancel={(event) => {
        event.preventDefault();
        cancel();
      }}
    >
      <div className="modal-dialog__icon modal-dialog__icon--danger">
        <Trash2 aria-hidden="true" size={20} />
      </div>
      {phase === "confirm-source" && confirmation ? (
        <>
          <p className="eyebrow">Permanent source deletion</p>
          <h2 id="deletion-dialog-title">Delete the original session?</h2>
          <p>
            <strong>{artifactSummary(confirmation.artifactCount)}</strong>{" "}
            This cannot be undone by AI Session Replay.
          </p>
          <div className="modal-dialog__warning">
            <AlertTriangle aria-hidden="true" size={17} />
            The indexed copy will also be removed and hidden from future refreshes.
          </div>
          <div className="modal-dialog__actions">
            <button
              ref={safeActionRef}
              type="button"
              className="button-secondary"
              onClick={() => {
                setConfirmation(null);
                setPhase("choose");
              }}
            >
              Go back
            </button>
            <button
              type="button"
              className="button-danger"
              onClick={() => void confirmSourceDeletion()}
            >
              Delete source and library
            </button>
          </div>
        </>
      ) : (
        <>
          <p className="eyebrow">Hide indexed session</p>
          <h2 id="deletion-dialog-title">Hide “{session.title}”?</h2>
          <p>
            The indexed copy will be removed and hidden from future refreshes. Your original
            session stays untouched by default.
          </p>
          <label className={`source-delete-option${session.sourcePresent ? "" : " source-delete-option--disabled"}`}>
            <input
              type="checkbox"
              aria-label={`Also delete this session from ${sourceLabel(session.source)}`}
              checked={deleteSource}
              disabled={!session.sourcePresent || busy}
              onChange={(event) => setDeleteSource(event.currentTarget.checked)}
            />
            <span>
              <strong>Also delete this session from {sourceLabel(session.source)}</strong>
              <small>
                {session.sourcePresent
                  ? "Requires a separate impact review before anything is deleted."
                  : "The source is no longer available, so only the indexed copy can be deleted."}
              </small>
            </span>
          </label>
          <div className="modal-dialog__status" aria-live="polite">
            {busy ? (
              <span><LoaderCircle className="spin" aria-hidden="true" size={14} />{phase === "preparing" ? "Checking source artifacts…" : deleteSource ? "Deleting source session…" : "Hiding session…"}</span>
            ) : failed ? (
              <span role="alert">Session could not be hidden. The indexed session remains available.</span>
            ) : null}
          </div>
          <div className="modal-dialog__actions">
            <button ref={safeActionRef} type="button" className="button-secondary" disabled={busy} onClick={cancel}>
              Cancel
            </button>
            <button
              type="button"
              className={deleteSource ? "button-danger" : "button-primary"}
              disabled={busy}
              onClick={() => void deleteOrReview()}
            >
              {deleteSource ? "Review source deletion" : "Hide from library"}
            </button>
          </div>
        </>
      )}
    </dialog>
  );
}

function artifactSummary(count: number): string {
  return `${count} source ${count === 1 ? "artifact" : "artifacts"} will be permanently deleted.`;
}
