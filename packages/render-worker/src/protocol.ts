import {
  validatePresentationPlan,
  type PresentationPlanV1,
} from "@ai-session-replay/replay-contract";
import {basename, dirname} from "node:path";

export const MAX_REQUEST_LINE_BYTES = 64 * 1_024 * 1_024;
export const MAX_REQUEST_STREAM_BYTES = MAX_REQUEST_LINE_BYTES + 2;
export const MAX_PROTOCOL_LINE_BYTES = 512;
export const MAX_PROTOCOL_BYTES = 128 * 1_024;
export const MAX_PROTOCOL_EVENTS = 256;

const JOB_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/u;
const WINDOWS_LOCAL_PATH_PATTERN = /^[A-Za-z]:[\\/]/u;
const WINDOWS_RESERVED_NAME_PATTERN = /^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)/iu;
const CONTROL_CHARACTER_PATTERN = /[\u0000-\u001F\u007F]/u;

export type WorkerFailureCode =
  | "INVALID_REQUEST"
  | "REQUEST_LIMIT_EXCEEDED"
  | "INVALID_RUNTIME"
  | "OUTPUT_NOT_AVAILABLE"
  | "RENDER_FAILED"
  | "MEDIA_ASSERTION_FAILED"
  | "PROTOCOL_LIMIT_EXCEEDED"
  | "WORKER_INTERNAL";

const FAILURE_CODES: ReadonlySet<WorkerFailureCode> = new Set([
  "INVALID_REQUEST",
  "REQUEST_LIMIT_EXCEEDED",
  "INVALID_RUNTIME",
  "OUTPUT_NOT_AVAILABLE",
  "RENDER_FAILED",
  "MEDIA_ASSERTION_FAILED",
  "PROTOCOL_LIMIT_EXCEEDED",
  "WORKER_INTERNAL",
]);

export class WorkerFailure extends Error {
  readonly code: WorkerFailureCode;

  constructor(code: WorkerFailureCode) {
    super(code);
    this.name = "WorkerFailure";
    this.code = code;
  }
}

export interface RenderRequest {
  readonly schemaVersion: 1;
  readonly type: "render";
  readonly jobId: string;
  readonly outputPath: string;
  readonly stagingPath: string;
  readonly plan: PresentationPlanV1;
}

export interface MediaSummary {
  readonly codec: "h264";
  readonly width: 1920;
  readonly height: 1080;
  readonly pixelFormat: "yuv420p";
  readonly durationMs: number;
  readonly frameCount: number;
}

export type ProtocolEvent =
  | Readonly<{
      schemaVersion: 1;
      type: "started";
      jobId: string;
      totalFrames: number;
    }>
  | Readonly<{
      schemaVersion: 1;
      type: "progress";
      jobId: string;
      renderedFrames: number;
      totalFrames: number;
    }>
  | Readonly<{
      schemaVersion: 1;
      type: "succeeded";
      jobId: string;
      media: MediaSummary;
      stagingSha256: string;
    }>
  | Readonly<{
      schemaVersion: 1;
      type: "failed";
      jobId: string;
      code: WorkerFailureCode;
    }>;

interface RequestLimits {
  readonly maxLineBytes: number;
  readonly maxStreamBytes: number;
}

interface ProtocolLimits {
  readonly maxEvents: number;
  readonly maxLineBytes: number;
  readonly maxBytes: number;
}

export function parseRenderRequestLine(line: string): RenderRequest {
  if (line.length === 0 || CONTROL_CHARACTER_PATTERN.test(line)) {
    throw new WorkerFailure("INVALID_REQUEST");
  }

  let input: unknown;
  try {
    input = JSON.parse(line) as unknown;
  } catch {
    throw new WorkerFailure("INVALID_REQUEST");
  }

  if (
    !isRecord(input) ||
    !hasExactKeys(input, [
      "schemaVersion",
      "type",
      "jobId",
      "outputPath",
      "stagingPath",
      "plan",
    ]) ||
    input.schemaVersion !== 1 ||
    input.type !== "render" ||
    typeof input.jobId !== "string" ||
    safeJobId(input.jobId) === "unassigned" ||
    !isWindowsLocalPath(input.outputPath) ||
    !input.outputPath.toLowerCase().endsWith(".mp4") ||
    !isWindowsLocalPath(input.stagingPath) ||
    !validStagingPath(input.stagingPath, input.outputPath)
  ) {
    throw new WorkerFailure("INVALID_REQUEST");
  }

  const plan = validatePresentationPlan(input.plan);
  if (!plan.ok) {
    throw new WorkerFailure("INVALID_REQUEST");
  }

  return {
    schemaVersion: 1,
    type: "render",
    jobId: input.jobId,
    outputPath: input.outputPath,
    stagingPath: input.stagingPath,
    plan: plan.value,
  };
}

function validStagingPath(stagingPath: string, outputPath: string): boolean {
  const normalizedParent = (value: string) => value.replaceAll("/", "\\").toLowerCase();
  if (normalizedParent(dirname(stagingPath)) !== normalizedParent(dirname(outputPath))) return false;
  return /^\.ai-session-replay-[0-9a-f]{32}\.partial\.mp4$/u.test(basename(stagingPath));
}

export async function readRenderRequest(
  source: AsyncIterable<Uint8Array | string>,
  limits: RequestLimits = {
    maxLineBytes: MAX_REQUEST_LINE_BYTES,
    maxStreamBytes: MAX_REQUEST_STREAM_BYTES,
  },
): Promise<RenderRequest> {
  if (
    !Number.isSafeInteger(limits.maxLineBytes) ||
    !Number.isSafeInteger(limits.maxStreamBytes) ||
    limits.maxLineBytes < 1 ||
    limits.maxStreamBytes < limits.maxLineBytes
  ) {
    throw new WorkerFailure("WORKER_INTERNAL");
  }

  const parts: Uint8Array[] = [];
  let streamBytes = 0;
  let lineBytes = 0;
  let newlineSeen = false;

  for await (const part of source) {
    const bytes = typeof part === "string" ? new TextEncoder().encode(part) : part;
    streamBytes += bytes.byteLength;
    if (streamBytes > limits.maxStreamBytes) {
      throw new WorkerFailure("REQUEST_LIMIT_EXCEEDED");
    }

    for (const byte of bytes) {
      if (newlineSeen) {
        throw new WorkerFailure("INVALID_REQUEST");
      }
      if (byte === 0x0a) {
        newlineSeen = true;
        continue;
      }
      lineBytes += 1;
      if (lineBytes > limits.maxLineBytes) {
        throw new WorkerFailure("REQUEST_LIMIT_EXCEEDED");
      }
    }
    parts.push(bytes);
  }

  const joined = new Uint8Array(streamBytes);
  let offset = 0;
  for (const part of parts) {
    joined.set(part, offset);
    offset += part.byteLength;
  }
  const payload = newlineSeen ? joined.subarray(0, joined.byteLength - 1) : joined;

  let line: string;
  try {
    line = new TextDecoder("utf-8", {fatal: true}).decode(payload);
  } catch {
    throw new WorkerFailure("INVALID_REQUEST");
  }
  if (line.endsWith("\r")) {
    line = line.slice(0, -1);
  }
  return parseRenderRequestLine(line);
}

export class ProtocolWriter {
  readonly #write: (line: string) => void;
  readonly #limits: ProtocolLimits;
  #bytes = 0;
  #events = 0;
  #terminal = false;

  constructor(
    write: (line: string) => void,
    limits: ProtocolLimits = {
      maxEvents: MAX_PROTOCOL_EVENTS,
      maxLineBytes: MAX_PROTOCOL_LINE_BYTES,
      maxBytes: MAX_PROTOCOL_BYTES,
    },
  ) {
    this.#write = write;
    this.#limits = limits;
  }

  started(jobId: string, totalFrames: number): void {
    this.#emit({schemaVersion: 1, type: "started", jobId, totalFrames}, false);
  }

  progress(jobId: string, renderedFrames: number, totalFrames: number): void {
    this.#emit(
      {schemaVersion: 1, type: "progress", jobId, renderedFrames, totalFrames},
      false,
    );
  }

  succeeded(jobId: string, media: MediaSummary, stagingSha256: string): void {
    if (!/^[0-9a-f]{64}$/u.test(stagingSha256)) {
      throw new WorkerFailure("PROTOCOL_LIMIT_EXCEEDED");
    }
    this.#emit({schemaVersion: 1, type: "succeeded", jobId, media, stagingSha256}, true);
  }

  failed(jobId: string, code: WorkerFailureCode): void {
    this.#emit(
      {
        schemaVersion: 1,
        type: "failed",
        jobId: safeJobId(jobId),
        code: FAILURE_CODES.has(code) ? code : "WORKER_INTERNAL",
      },
      true,
    );
  }

  #emit(event: ProtocolEvent, terminal: boolean): void {
    if (this.#terminal) {
      return;
    }
    if (this.#events >= this.#limits.maxEvents - (terminal ? 0 : 1)) {
      throw new WorkerFailure("PROTOCOL_LIMIT_EXCEEDED");
    }

    const line = `${JSON.stringify(event)}\n`;
    const lineBytes = new TextEncoder().encode(line).byteLength;
    if (
      lineBytes > this.#limits.maxLineBytes ||
      this.#bytes + lineBytes > this.#limits.maxBytes
    ) {
      throw new WorkerFailure("PROTOCOL_LIMIT_EXCEEDED");
    }

    this.#write(line);
    this.#events += 1;
    this.#bytes += lineBytes;
    this.#terminal = terminal;
  }
}

export function classifyWorkerError(error: unknown): WorkerFailureCode {
  return error instanceof WorkerFailure && FAILURE_CODES.has(error.code)
    ? error.code
    : "WORKER_INTERNAL";
}

export function safeJobId(value: unknown): string {
  return typeof value === "string" && JOB_ID_PATTERN.test(value)
    ? value
    : "unassigned";
}

export function isWindowsLocalPath(value: unknown): value is string {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > 32_767 ||
    CONTROL_CHARACTER_PATTERN.test(value) ||
    !WINDOWS_LOCAL_PATH_PATTERN.test(value) ||
    value.slice(2).includes(":")
  ) {
    return false;
  }

  return value
    .slice(3)
    .split(/[\\/]/u)
    .every(
      (part) =>
        part.length > 0 &&
        !/[<>"|?*]/u.test(part) &&
        !/[. ]$/u.test(part) &&
        !WINDOWS_RESERVED_NAME_PATTERN.test(part),
    );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
): boolean {
  const keys = Object.keys(value).sort();
  const wanted = [...expected].sort();
  return (
    keys.length === wanted.length &&
    keys.every((key, index) => key === wanted[index])
  );
}
