export function exportWasCancelled(error: unknown): boolean {
  return errorCode(error) === "EXPORT_CANCELLED";
}

export function exportFailureMessage(error: unknown): string {
  switch (errorCode(error)) {
    case "EXPORT_CANCELLED":
      return "Export cancelled";
    case "INVALID_REQUEST":
      return "The export request is no longer valid. Reopen the session and try again.";
    case "EXPORT_BUSY":
      return "Export failed because another export is active.";
    case "EXPORT_RUNTIME_UNAVAILABLE":
      return "Export runtime unavailable. Rebuild or reinstall the application runtime.";
    case "EXPORT_OUTPUT_UNAVAILABLE":
      return "The export destination is unavailable. Choose a writable local folder and a new filename.";
    case "EXPORT_OUTPUT_EXISTS":
      return "That MP4 already exists. Choose another filename; existing videos are never overwritten.";
    case "EXPORT_OUTPUT_FORMAT":
      return "Choose a destination filename ending in .mp4.";
    case "EXPORT_OUTPUT_UNSAFE":
      return "Choose a local destination outside indexed session folders.";
    case "EXPORT_PROCESS_FAILED":
      return "The export process could not start or finish. Restart the app and try again.";
    case "EXPORT_PROTOCOL_FAILED":
      return "The renderer returned an invalid response. Rebuild or reinstall the application runtime.";
    case "EXPORT_CONTENT_TOO_LARGE":
      return "The selected conversation is too large to export.";
    case "EXPORT_WORKER_FAILED":
      return "The export worker failed. Restart the app and try again.";
    case "EXPORT_RENDER_FAILED":
      return "Video rendering failed. Try again or reduce the selected content.";
    case "EXPORT_VERIFICATION_FAILED":
      return "The MP4 was created but failed verification and was not published.";
    default:
      return "Export failed. The renderer could not create the MP4.";
  }
}

function errorCode(error: unknown): unknown {
  if (typeof error === "object" && error !== null && "code" in error) return error.code;
  const message = typeof error === "string" ? error : error instanceof Error ? error.message : null;
  if (message === null) return null;
  try {
    const parsed: unknown = JSON.parse(message);
    if (typeof parsed === "object" && parsed !== null && "code" in parsed) return parsed.code;
  } catch {
    // Some Tauri runtimes reject with the safe code as a plain string.
  }
  return message.match(/\b[A-Z][A-Z0-9_]+\b/)?.[0] ?? null;
}

export function defaultExportFileName(nowMs = Date.now()): string {
  return `session-replay-${Math.max(0, Math.trunc(nowMs)).toString(36)}.mp4`;
}
