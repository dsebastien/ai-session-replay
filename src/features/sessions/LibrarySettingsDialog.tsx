import {AlertTriangle, DatabaseZap, LoaderCircle} from "lucide-react";
import {useEffect, useRef, useState} from "react";
import type {IndexedSessionClient} from "../../lib/tauri/indexed-sessions";
import {useModalDialog} from "./useModalDialog";

interface LibrarySettingsDialogProps {
  readonly client: IndexedSessionClient;
  readonly refreshRunning: boolean;
  readonly onClose: () => void;
  readonly onReset: () => void;
}

type ResetPhase = "settings" | "confirm" | "resetting";

export function LibrarySettingsDialog({
  client,
  refreshRunning,
  onClose,
  onReset,
}: LibrarySettingsDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const safeActionRef = useRef<HTMLButtonElement>(null);
  const [phase, setPhase] = useState<ResetPhase>("settings");
  const [failed, setFailed] = useState(false);
  useModalDialog(dialogRef, safeActionRef);
  useEffect(() => {
    if (phase !== "resetting") safeActionRef.current?.focus();
  }, [phase]);

  const busy = phase === "resetting";
  const close = () => {
    if (!busy) onClose();
  };
  const reset = async () => {
    setFailed(false);
    setPhase("resetting");
    try {
      await client.resetLocalDatabase();
      onReset();
    } catch {
      setFailed(true);
      setPhase("confirm");
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="modal-dialog settings-dialog"
      aria-labelledby="settings-dialog-title"
      onCancel={(event) => {
        event.preventDefault();
        close();
      }}
    >
      <div className="modal-dialog__icon modal-dialog__icon--danger">
        <DatabaseZap aria-hidden="true" size={20} />
      </div>
      {phase === "settings" ? (
        <>
          <p className="eyebrow">Local data</p>
          <h2 id="settings-dialog-title">Library settings</h2>
          <p>Manage the indexed data stored by AI Session Replay on this device.</p>
          <section className="settings-danger" aria-labelledby="reset-library-title">
            <h3 id="reset-library-title">Reset local database</h3>
            <p>
              Clear indexed sessions, preferences, refresh history, and hidden-source records.
              Original source files stay untouched.
            </p>
            <button
              type="button"
              className="button-danger"
              disabled={refreshRunning}
              onClick={() => setPhase("confirm")}
            >
              Reset local database
            </button>
            <span className="settings-danger__status">
              {refreshRunning ? "Wait for the current refresh to finish." : "A confirmation is required."}
            </span>
          </section>
          <div className="modal-dialog__actions">
            <button ref={safeActionRef} type="button" className="button-primary" onClick={close}>
              Done
            </button>
          </div>
        </>
      ) : (
        <>
          <p className="eyebrow">Permanent local reset</p>
          <h2 id="settings-dialog-title">Clear the indexed library?</h2>
          <p>
            This removes all app-owned sessions and settings. Your original Claude Code, Codex,
            GitHub Copilot CLI, and VS Code Copilot Chat files will not be changed. A later refresh can index them again.
          </p>
          <div className="modal-dialog__warning">
            <AlertTriangle aria-hidden="true" size={17} />
            Hidden-source records are also cleared, so retained sources may return after refresh.
          </div>
          <div className="modal-dialog__status" aria-live="polite">
            {busy ? (
              <span><LoaderCircle className="spin" aria-hidden="true" size={14} />Resetting local database…</span>
            ) : failed ? (
              <span role="alert">The local database could not be reset. Existing data was retained.</span>
            ) : null}
          </div>
          <div className="modal-dialog__actions">
            <button
              ref={safeActionRef}
              type="button"
              className="button-secondary"
              disabled={busy}
              onClick={() => setPhase("settings")}
            >
              Go back
            </button>
            <button
              type="button"
              className="button-danger"
              disabled={busy}
              onClick={() => void reset()}
            >
              Permanently reset local database
            </button>
          </div>
        </>
      )}
    </dialog>
  );
}
