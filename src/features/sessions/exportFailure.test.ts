import {describe, expect, it} from "vitest";
import {defaultExportFileName, exportFailureMessage, exportWasCancelled} from "./exportFailure";

describe("export failure guidance", () => {
  it.each([
    ["EXPORT_OUTPUT_EXISTS", "That MP4 already exists. Choose another filename; existing videos are never overwritten."],
    ["EXPORT_OUTPUT_FORMAT", "Choose a destination filename ending in .mp4."],
    ["EXPORT_OUTPUT_UNSAFE", "Choose a local destination outside indexed session folders."],
    ["EXPORT_OUTPUT_UNAVAILABLE", "The export destination is unavailable. Choose a writable local folder and a new filename."],
    ["EXPORT_RUNTIME_UNAVAILABLE", "Export runtime unavailable. Rebuild or reinstall the application runtime."],
    ["EXPORT_PROCESS_FAILED", "The export process could not start or finish. Restart the app and try again."],
    ["EXPORT_PROTOCOL_FAILED", "The renderer returned an invalid response. Rebuild or reinstall the application runtime."],
    ["EXPORT_CONTENT_TOO_LARGE", "The selected conversation is too large to export."],
    ["EXPORT_RENDER_FAILED", "Video rendering failed. Try again or reduce the selected content."],
    ["EXPORT_VERIFICATION_FAILED", "The MP4 was created but failed verification and was not published."],
  ])("maps %s to actionable guidance", (code, expected) => {
    expect(exportFailureMessage({code})).toBe(expected);
  });

  it("recognizes structured and serialized cancellation errors", () => {
    expect(exportWasCancelled({code: "EXPORT_CANCELLED"})).toBe(true);
    expect(exportWasCancelled('{"code":"EXPORT_CANCELLED"}')).toBe(true);
  });

  it("uses a valid changing MP4 filename by default", () => {
    expect(defaultExportFileName(1_000)).toBe("session-replay-rs.mp4");
    expect(defaultExportFileName(1_001)).not.toBe(defaultExportFileName(1_000));
  });
});
