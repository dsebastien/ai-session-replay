import {ChevronLeft, ChevronRight, Maximize2, Pause, Play, X} from "lucide-react";
import {useEffect, useMemo, useReducer, useRef, useState} from "react";
import {save} from "@tauri-apps/plugin-dialog";
import {
  initialPresentationState,
  playbackDelayUntil,
  presentationIndexAtElapsed,
  reducePresentation,
} from "../../../packages/replay-engine/src";
import type {PresentationPlanV1} from "../../../packages/replay-contract/src";
import {
  PresentationFrame,
} from "../../../packages/remotion-composition/src/ConversationStage";
import {useModalDialog} from "./useModalDialog";
import type {IndexedSessionClient} from "../../lib/tauri/indexed-sessions";
import {defaultExportFileName, exportFailureMessage, exportWasCancelled} from "./exportFailure";
import {
  presentationFullscreenActive,
  setPresentationFullscreen,
  usesNativePresentationFullscreen,
} from "./presentationFullscreen";
import {ToastNotification} from "./ToastNotification";

interface PresentationDialogProps {
  readonly plan: PresentationPlanV1;
  readonly onClose: () => void;
  readonly client: IndexedSessionClient;
}

export function PresentationDialog({plan, onClose, client}: PresentationDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const playbackOriginRef = useRef<number | null>(null);
  const activeExportRef = useRef<string | null>(null);
  const nativeFullscreenOwnedRef = useRef(false);
  const [playback, dispatch] = useReducer(
    (state: ReturnType<typeof initialPresentationState>, action: Parameters<typeof reducePresentation>[1]) =>
      reducePresentation(state, action, plan.entries.length),
    plan.entries.length,
    initialPresentationState,
  );
  useModalDialog(dialogRef, stageRef);
  const [exportState, setExportState] = useState<"idle" | "exporting" | "succeeded" | "failed" | "cancelled">("idle");
  const [exportJobId, setExportJobId] = useState<string | null>(null);
  const [completedExportJobId, setCompletedExportJobId] = useState<string | null>(null);
  const [exportProgress, setExportProgress] = useState<Readonly<{rendered: number; total: number}> | null>(null);
  const [revealFailed, setRevealFailed] = useState(false);
  const [cancelFailed, setCancelFailed] = useState(false);
  const [exportFailure, setExportFailure] = useState<string | null>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [fullscreenError, setFullscreenError] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const revealTimes = useMemo(() => plan.entries.map(({revealAtMs}) => revealAtMs), [plan.entries]);
  activeExportRef.current = exportJobId;

  const exportVideo = async () => {
    let unsubscribe: () => void = () => {};
    try {
      const outputPath = await save({
        title: "Export presentation",
        defaultPath: defaultExportFileName(),
        filters: [{name: "MP4 video", extensions: ["mp4"]}],
      });
      if (!outputPath) return;
      const jobId = `export_${Date.now().toString(36)}`;
      setExportJobId(jobId);
      activeExportRef.current = jobId;
      setCompletedExportJobId(null);
      setExportProgress(null);
      setRevealFailed(false);
      setCancelFailed(false);
      setExportFailure(null);
      setExportState("exporting");
      unsubscribe = await client.subscribeToExportProgress(jobId, (progress) => {
        setExportProgress({rendered: progress.renderedFrames, total: progress.totalFrames});
      });
      await client.exportPresentation(plan.planId, outputPath, jobId);
      setCompletedExportJobId(jobId);
      setExportState("succeeded");
    } catch (error) {
      setExportFailure(exportFailureMessage(error));
      setExportState(exportWasCancelled(error) ? "cancelled" : "failed");
    } finally {
      unsubscribe();
      activeExportRef.current = null;
      setExportJobId(null);
    }
  };

  const closePresentation = async () => {
    const jobId = activeExportRef.current;
    if (jobId) {
      try {
        await client.cancelExport(jobId);
      } catch {
        setCancelFailed(true);
        return;
      }
      activeExportRef.current = null;
    }
    const dialog = dialogRef.current;
    if (dialog) {
      try {
        if (nativeFullscreenOwnedRef.current) {
          nativeFullscreenOwnedRef.current = false;
          await setPresentationFullscreen(dialog, false);
        } else if (!usesNativePresentationFullscreen() && document.fullscreenElement === dialog) {
          await setPresentationFullscreen(dialog, false);
        }
      } catch {
        // Closing the presentation must remain possible if the host rejects a fullscreen transition.
      }
    }
    onClose();
  };

  const cancelActiveExport = async () => {
    if (!exportJobId) return;
    try {
      await client.cancelExport(exportJobId);
    } catch {
      setCancelFailed(true);
    }
  };

  const revealCompletedExport = async () => {
    if (!completedExportJobId) return;
    setRevealFailed(false);
    try {
      await client.revealExport(completedExportJobId);
    } catch {
      setRevealFailed(true);
    }
  };

  const toastMessage = fullscreenError
    ? "Fullscreen unavailable"
    : revealFailed
        ? "Export complete; folder unavailable"
        : exportState === "failed"
          ? exportFailure
          : null;

  const toggleFullscreen = async () => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    setFullscreenError(false);
    try {
      const nextFullscreen = !(await presentationFullscreenActive(dialog));
      await setPresentationFullscreen(dialog, nextFullscreen);
      nativeFullscreenOwnedRef.current = usesNativePresentationFullscreen() && nextFullscreen;
      setFullscreen(nextFullscreen);
      stageRef.current?.focus({preventScroll: true});
    } catch {
      setFullscreenError(true);
    }
  };

  useEffect(() => {
    if (!usesNativePresentationFullscreen()) return;
    const dialog = dialogRef.current;
    if (!dialog) return;
    let disposed = false;
    void (async () => {
      try {
        const alreadyFullscreen = await presentationFullscreenActive(dialog);
        if (disposed) return;
        if (alreadyFullscreen) {
          setFullscreen(true);
          return;
        }
        await setPresentationFullscreen(dialog, true);
        if (disposed) {
          await setPresentationFullscreen(dialog, false);
          return;
        }
        nativeFullscreenOwnedRef.current = true;
        setFullscreen(true);
      } catch {
        if (!disposed) setFullscreenError(true);
      }
    })();
    return () => {
      disposed = true;
      if (!nativeFullscreenOwnedRef.current) return;
      nativeFullscreenOwnedRef.current = false;
      void setPresentationFullscreen(dialog, false).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    const query = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    if (!query) return;
    const update = () => setReducedMotion(query.matches);
    update();
    query.addEventListener?.("change", update);
    return () => query.removeEventListener?.("change", update);
  }, []);

  useEffect(() => {
    const update = () => {
      if (!usesNativePresentationFullscreen()) {
        setFullscreen(document.fullscreenElement === dialogRef.current);
      }
    };
    document.addEventListener("fullscreenchange", update);
    return () => document.removeEventListener("fullscreenchange", update);
  }, []);

  useEffect(() => {
    if (reducedMotion) dispatch({type: "pause"});
  }, [reducedMotion]);

  useEffect(() => () => {
    const jobId = activeExportRef.current;
    if (jobId) void client.cancelExport(jobId).catch(() => undefined);
  }, [client]);

  useEffect(() => {
    if (playback.status !== "playing") {
      playbackOriginRef.current = null;
      return;
    }
    const current = plan.entries[playback.index];
    const now = performance.now();
    playbackOriginRef.current ??= now - (current?.revealAtMs ?? 0);
    const nextBoundary = plan.entries[playback.index + 1]?.revealAtMs ?? plan.durationMs;
    const delay = playbackDelayUntil(playbackOriginRef.current, nextBoundary, now);
    const timer = window.setTimeout(() => {
      const elapsedMs = performance.now() - (playbackOriginRef.current ?? performance.now());
      dispatch({
        type: "sync",
        index: presentationIndexAtElapsed(revealTimes, elapsedMs),
        complete: elapsedMs >= plan.durationMs,
      });
    }, delay);
    return () => window.clearTimeout(timer);
  }, [plan.durationMs, plan.entries, playback, revealTimes]);

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDialogElement>) => {
    if (event.target instanceof HTMLElement && event.target.closest("button, input, select, textarea, a")) {
      return;
    }
    const action =
      event.key === " " ? "toggle"
        : event.key === "ArrowLeft" || event.key === "ArrowUp" ? "previous"
          : event.key === "ArrowRight" || event.key === "ArrowDown" ? "next"
            : event.key === "Home" ? "first"
              : event.key === "End" ? "last"
                : null;
    if (action && !(action === "toggle" && reducedMotion)) {
      event.preventDefault();
      dispatch({type: action});
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="presentation-dialog"
      aria-labelledby="presentation-title"
      onCancel={(event) => {
        event.preventDefault();
        void closePresentation();
      }}
      onKeyDown={handleKeyDown}
    >
      {toastMessage ? (
        <ToastNotification key={toastMessage} message={toastMessage} />
      ) : null}
      <header>
        <div>
          <p className="eyebrow">Presentation</p>
          <h2 id="presentation-title">{plan.sessionTitle}</h2>
        </div>
        <button type="button" className="toolbar-button" aria-label="Close presentation" onClick={() => void closePresentation()}>
          <X aria-hidden="true" size={16} />Close
        </button>
      </header>
      <div
        ref={stageRef}
        className="presentation-stage"
        role="region"
        aria-label="Presentation entry"
        aria-live="polite"
        tabIndex={0}
      >
        <PresentationFrame plan={plan} index={playback.index} interactive />
      </div>
      <footer className="presentation-controls">
        <div className="presentation-controls__transport">
          <button type="button" aria-label="First entry" onClick={() => dispatch({type: "first"})}>First</button>
          <button type="button" aria-label="Previous entry" onClick={() => dispatch({type: "previous"})}><ChevronLeft aria-hidden="true" size={16} /></button>
          <button type="button" disabled={reducedMotion} aria-label={playback.status === "playing" ? "Pause" : "Play"} onClick={() => dispatch({type: "toggle"})}>
            {playback.status === "playing" ? <Pause aria-hidden="true" size={16} /> : <Play aria-hidden="true" size={16} />}
          </button>
          <button type="button" aria-label="Next entry" onClick={() => dispatch({type: "next"})}><ChevronRight aria-hidden="true" size={16} /></button>
          <button type="button" aria-label="Last entry" onClick={() => dispatch({type: "last"})}>Last</button>
          <span>{playback.index + 1} / {plan.entries.length}</span>
          <button type="button" aria-label={fullscreen ? "Exit fullscreen" : "Enter fullscreen"} onClick={() => void toggleFullscreen()}>
            <Maximize2 aria-hidden="true" size={16} />{fullscreen ? "Window" : "Full screen"}
          </button>
        </div>
        <div className="presentation-controls__export">
          <span>{formatDuration(plan.durationMs)} · 1080p MP4</span>
          <button type="button" disabled={exportState === "exporting"} onClick={() => void exportVideo()}>
            {exportState === "exporting" ? "Exporting…" : "Export MP4"}
          </button>
          <button
            type="button"
            className={exportState === "exporting" || exportState === "succeeded" ? "" : "presentation-controls__placeholder"}
            aria-hidden={exportState !== "exporting" && exportState !== "succeeded"}
            tabIndex={exportState === "exporting" || exportState === "succeeded" ? 0 : -1}
            onClick={() => exportState === "exporting" ? void cancelActiveExport() : void revealCompletedExport()}
          >
            {exportState === "exporting" ? "Cancel export" : "Show in folder"}
          </button>
          <span aria-live="polite">
            {exportState === "exporting"
              ? cancelFailed
                ? "Cancellation unavailable; export continues"
                : exportProgress
                  ? `Exporting ${Math.floor((exportProgress.rendered / exportProgress.total) * 100)}%`
                  : "Preparing export…"
              : exportState === "succeeded" ? "Export complete"
                : exportState === "cancelled" ? "Export cancelled"
                  : reducedMotion ? "Reduced motion active" : null}
          </span>
          <progress
            aria-label="Export progress"
            className={exportState === "exporting" ? "" : "presentation-controls__placeholder"}
            max={exportProgress?.total ?? 1}
            value={exportProgress?.rendered ?? 0}
          />
        </div>
      </footer>
    </dialog>
  );
}

function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.max(1, Math.round(milliseconds / 1_000));
  return `${Math.floor(totalSeconds / 60)}:${String(totalSeconds % 60).padStart(2, "0")}`;
}
