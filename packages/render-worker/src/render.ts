import {frameCount} from "@ai-session-replay/replay-engine";
import {
  openBrowser,
  renderMedia,
  RenderInternals,
  selectComposition,
  type HeadlessBrowser,
} from "@remotion/renderer";
import {spawn} from "node:child_process";
import {createHash} from "node:crypto";
import {createReadStream} from "node:fs";
import {lstat, open, realpath, stat, unlink} from "node:fs/promises";
import {createServer} from "node:net";
import {basename, dirname, isAbsolute, join, relative, sep} from "node:path";
import type {MediaSummary, RenderRequest} from "./protocol";
import {isWindowsLocalPath, WorkerFailure} from "./protocol";

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "[::1]", "localhost"]);
const COMPOSITION_ID = "TerminalReplay";
const MAX_RUNTIME_LAYOUT_BYTES = 16 * 1_024;
const MAX_FFPROBE_STDOUT_BYTES = 64 * 1_024;
const MAX_FFPROBE_STDERR_BYTES = 16 * 1_024;
const FFPROBE_TIMEOUT_MS = 30_000;

const EXPECTED_LAYOUT = {
  schemaVersion: 1,
  platform: "windows-x64",
  versions: {
    bun: "1.4.0",
    chromeHeadlessShell: "149.0.7790.0",
    remotion: "4.0.516",
  },
  paths: {
    bunExecutable: "bun/bun.exe",
    browserExecutable: "chrome-headless-shell/chrome-headless-shell.exe",
    binariesDirectory: "remotion",
    composition: "composition",
  },
  archiveSha256: {
    bun: "e6f093d39da486b20262ca8cdd5ed6a9e8bc9c2f275b78e6d3a0c5b28cc95901",
    chromeHeadlessShell:
      "8a112c0e768907ff382ff994dcde35cdbd0e1f5a3a475da93d3a83e3260e78c2",
    remotionCompositor:
      "946cdc35b2d08ca83e2d4bf66f1c23656cd321ce90d8bdfee72d2ff10d8c502e",
  },
} as const;

export interface PreparedRenderJob {
  readonly request: RenderRequest;
  readonly totalFrames: number;
  readonly expectedDurationMs: number;
  readonly paths: Readonly<{
    browserExecutable: string;
    binariesDirectory: string;
    compositionDirectory: string;
    ffprobeExecutable: string;
    outputPath: string;
    stagingPath: string;
  }>;
}

export interface RenderedMedia {
  readonly media: MediaSummary;
  readonly stagingSha256: string;
}

interface DevtoolsSessionLike {
  on(event: string, handler: (event: unknown) => void): unknown;
  send(method: string, params?: unknown): Promise<unknown>;
}

interface BrowserPageLike {
  _client(): DevtoolsSessionLike;
}

export async function prepareRenderJob(
  request: RenderRequest,
  runtimeDirectory: string,
): Promise<PreparedRenderJob> {
  if (
    process.platform !== "win32" ||
    process.arch !== "x64" ||
    !isWindowsLocalPath(runtimeDirectory)
  ) {
    throw new WorkerFailure("INVALID_RUNTIME");
  }

  const runtimeRoot = await canonicalDirectory(
    runtimeDirectory,
    "INVALID_RUNTIME",
  );
  const layoutPath = await regularFileWithin(
    runtimeRoot,
    join(runtimeRoot, "runtime-layout.json"),
  );
  let layout: unknown;
  try {
    layout = JSON.parse(
      await readUtf8FileBounded(layoutPath, MAX_RUNTIME_LAYOUT_BYTES),
    ) as unknown;
  } catch {
    throw new WorkerFailure("INVALID_RUNTIME");
  }
  if (!deepEqual(layout, EXPECTED_LAYOUT)) {
    throw new WorkerFailure("INVALID_RUNTIME");
  }

  const browserExecutable = await regularFileWithin(
    runtimeRoot,
    join(runtimeRoot, ...EXPECTED_LAYOUT.paths.browserExecutable.split("/")),
  );
  const compositionDirectory = await canonicalDirectory(
    join(runtimeRoot, EXPECTED_LAYOUT.paths.composition),
    "INVALID_RUNTIME",
    runtimeRoot,
  );
  await regularFileWithin(compositionDirectory, join(compositionDirectory, "index.html"));
  const binariesDirectory = await canonicalDirectory(
    join(runtimeRoot, EXPECTED_LAYOUT.paths.binariesDirectory),
    "INVALID_RUNTIME",
    runtimeRoot,
  );
  await regularFileWithin(binariesDirectory, join(binariesDirectory, "remotion.exe"));
  await regularFileWithin(binariesDirectory, join(binariesDirectory, "ffmpeg.exe"));
  const ffprobeExecutable = await regularFileWithin(
    binariesDirectory,
    join(binariesDirectory, "ffprobe.exe"),
  );

  const outputParent = await canonicalDirectory(
    dirname(request.outputPath),
    "OUTPUT_NOT_AVAILABLE",
  );
  const outputPath = join(outputParent, basename(request.outputPath));
  const stagingPath = join(outputParent, basename(request.stagingPath));
  if (
    isInside(runtimeRoot, outputPath) ||
    isInside(runtimeRoot, stagingPath) ||
    await pathExists(outputPath) ||
    await pathExists(stagingPath)
  ) {
    throw new WorkerFailure("OUTPUT_NOT_AVAILABLE");
  }

  const totalFrames = frameCount(request.plan.durationMs, request.plan.fps);
  return {
    request,
    totalFrames,
    expectedDurationMs: (totalFrames / request.plan.fps) * 1_000,
    paths: {
      browserExecutable,
      binariesDirectory,
      compositionDirectory,
      ffprobeExecutable,
      outputPath,
      stagingPath,
    },
  };
}

export async function renderPreparedJob(
  job: PreparedRenderJob,
  onProgress: (renderedFrames: number, totalFrames: number) => void,
): Promise<RenderedMedia> {
  forceRemotionLoopbackBinding();
  let browser: HeadlessBrowser | undefined;
  let failure: unknown;
  let media: MediaSummary | undefined;
  let stagingSha256: string | undefined;

  try {
    const loopbackPort = await reserveLoopbackPort();
    // Remotion 4.0.516: https://www.remotion.dev/docs/renderer/open-browser
    browser = await openBrowser("chrome", {
      browserExecutable: job.paths.browserExecutable,
      chromeMode: "headless-shell",
      chromiumOptions: {headless: true},
      logLevel: "error",
    });
    installLoopbackNetworkPolicy(browser, loopbackPort);

    const inputProps = {plan: job.request.plan};
    // https://www.remotion.dev/docs/renderer/select-composition
    const composition = await selectComposition({
      serveUrl: job.paths.compositionDirectory,
      id: COMPOSITION_ID,
      inputProps,
      puppeteerInstance: browser,
      browserExecutable: job.paths.browserExecutable,
      binariesDirectory: job.paths.binariesDirectory,
      chromeMode: "headless-shell",
      logLevel: "error",
      port: loopbackPort,
    });
    if (
      composition.id !== COMPOSITION_ID ||
      composition.width !== job.request.plan.width ||
      composition.height !== job.request.plan.height ||
      composition.fps !== job.request.plan.fps ||
      composition.durationInFrames !== job.totalFrames
    ) {
      throw new WorkerFailure("INVALID_RUNTIME");
    }

    let lastProgressBucket = -1;
    let lastRenderedFrames = -1;
    // https://www.remotion.dev/docs/renderer/render-media
    await renderMedia({
      serveUrl: job.paths.compositionDirectory,
      composition,
      inputProps,
      codec: "h264",
      imageFormat: "png",
      pixelFormat: "yuv420p",
      outputLocation: job.paths.stagingPath,
      overwrite: false,
      puppeteerInstance: browser,
      browserExecutable: job.paths.browserExecutable,
      binariesDirectory: job.paths.binariesDirectory,
      chromeMode: "headless-shell",
      chromiumOptions: {headless: true},
      concurrency: 1,
      disallowParallelEncoding: true,
      logLevel: "error",
      port: loopbackPort,
      onBrowserLog: () => undefined,
      onProgress: (progress) => {
        const bucket = Math.floor(progress.progress * 100);
        const renderedFrames = Math.min(
          job.totalFrames,
          Math.max(progress.renderedFrames, progress.encodedFrames),
        );
        if (
          bucket <= lastProgressBucket ||
          renderedFrames <= lastRenderedFrames
        ) {
          return;
        }
        lastProgressBucket = bucket;
        lastRenderedFrames = renderedFrames;
        onProgress(renderedFrames, job.totalFrames);
      },
    });
    const beforeInspection = await sha256File(job.paths.stagingPath);
    media = await inspectMedia(job, job.paths.stagingPath);
    stagingSha256 = await sha256File(job.paths.stagingPath);
    if (stagingSha256 !== beforeInspection) {
      throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
    }
  } catch (error) {
    failure = error;
  }

  if (browser !== undefined) {
    try {
      await browser.close({silent: true});
    } catch (error) {
      failure ??= error;
    }
  }

  if (failure !== undefined || media === undefined || stagingSha256 === undefined) {
    await removeStagingOutput(job.paths.stagingPath);
    if (failure instanceof WorkerFailure) {
      throw failure;
    }
    throw new WorkerFailure("RENDER_FAILED");
  }
  return {
    media,
    stagingSha256,
  };
}

export async function removeStagingOutput(stagingPath: string): Promise<void> {
  await unlink(stagingPath).catch(() => undefined);
}

async function sha256File(path: string): Promise<string> {
  const digest = createHash("sha256");
  try {
    for await (const chunk of createReadStream(path)) digest.update(chunk);
    return digest.digest("hex");
  } catch {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }
}

export function forceRemotionLoopbackBinding(): Readonly<{
  host: string;
  hostsToTry: readonly string[];
}> {
  // The pinned renderer caches and reuses this object for every local server.
  const config = RenderInternals.getPortConfig(true);
  config.host = "127.0.0.1";
  config.hostsToTry = ["127.0.0.1"];
  if (config.host !== "127.0.0.1" || config.hostsToTry.length !== 1) {
    throw new WorkerFailure("INVALID_RUNTIME");
  }
  return config;
}

export function installLoopbackNetworkPolicy(browser: HeadlessBrowser, allowedPort: number): void {
  const newPage = browser.newPage.bind(browser);
  browser.newPage = async (options) => {
    const page = await newPage(options);
    await restrictBrowserPage(page, allowedPort);
    return page;
  };
}

export async function restrictBrowserPage(page: BrowserPageLike, allowedPort: number): Promise<void> {
  const client = page._client();
  client.on("Fetch.requestPaused", (event) => {
    if (!isRecord(event) || typeof event.requestId !== "string") {
      return;
    }
    const url = isRecord(event.request) && typeof event.request.url === "string"
      ? event.request.url
      : "";
    const operation = isBrowserUrlAllowed(url, allowedPort)
      ? client.send("Fetch.continueRequest", {requestId: event.requestId})
      : client.send("Fetch.failRequest", {
          errorReason: "BlockedByClient",
          requestId: event.requestId,
        });
    void operation.catch(() => undefined);
  });
  await client.send("Fetch.enable", {
    patterns: [{requestStage: "Request", urlPattern: "*"}],
  });
}

export function isBrowserUrlAllowed(rawUrl: string, allowedPort: number): boolean {
  if (!Number.isSafeInteger(allowedPort) || allowedPort < 1 || allowedPort > 65_535) return false;
  let url: URL;
  try {
    url = new URL(rawUrl);
  } catch {
    return false;
  }

  if (url.protocol === "data:") {
    return true;
  }
  if (url.protocol === "blob:") {
    return isBrowserUrlAllowed(url.pathname, allowedPort);
  }
  return url.protocol === "http:" &&
    LOOPBACK_HOSTS.has(url.hostname.toLowerCase()) &&
    url.port === String(allowedPort);
}

export async function reserveLoopbackPort(): Promise<number> {
  const server = createServer();
  return new Promise<number>((resolvePort, rejectPort) => {
    server.once("error", rejectPort);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (typeof address === "object" && address !== null && address.port > 0) {
        const port = address.port;
        server.close((error) => error ? rejectPort(error) : resolvePort(port));
      } else {
        server.close(() => rejectPort(new Error("loopback port unavailable")));
      }
    });
  });
}

export function assertMediaMetadata(
  rawMetadata: string,
  expected: Readonly<{durationMs: number; frameCount: number}>,
): MediaSummary {
  let input: unknown;
  try {
    input = JSON.parse(rawMetadata) as unknown;
  } catch {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }

  if (!isRecord(input) || !Array.isArray(input.streams) || input.streams.length !== 1) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }
  const stream = input.streams[0];
  if (!isRecord(stream)) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }

  const frameCount = parseUnsignedInteger(stream.nb_read_frames);
  const durationSeconds = parseFiniteDecimal(stream.duration);
  const durationMs = durationSeconds * 1_000;
  if (
    stream.codec_name !== "h264" ||
    stream.width !== 1_920 ||
    stream.height !== 1_080 ||
    stream.pix_fmt !== "yuv420p" ||
    frameCount !== expected.frameCount ||
    Math.abs(durationMs - expected.durationMs) > 1
  ) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }

  return {
    codec: "h264",
    width: 1_920,
    height: 1_080,
    pixelFormat: "yuv420p",
    durationMs: Math.round(durationMs),
    frameCount,
  };
}

function parseUnsignedInteger(value: unknown): number {
  if (typeof value !== "string" || !/^(?:0|[1-9]\d*)$/u.test(value)) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }
  return parsed;
}

function parseFiniteDecimal(value: unknown): number {
  if (typeof value !== "string" || !/^\d+(?:\.\d+)?$/u.test(value)) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }
  return parsed;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function inspectMedia(job: PreparedRenderJob, mediaPath: string): Promise<MediaSummary> {
  const output = await stat(mediaPath).catch(() => null);
  if (output === null || !output.isFile() || output.size === 0) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }

  const child = spawn(
    job.paths.ffprobeExecutable,
    [
      "-v",
      "error",
      "-select_streams",
      "v:0",
      "-count_frames",
      "-show_entries",
      "stream=codec_name,width,height,pix_fmt,nb_read_frames,duration",
      "-of",
      "json",
      mediaPath,
    ],
    {
      env: {
        PATH: "",
        SystemRoot: process.env.SystemRoot,
        TEMP: process.env.TEMP,
        TMP: process.env.TMP,
        WINDIR: process.env.WINDIR,
      },
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    },
  );

  let timedOut = false;
  const timeout = setTimeout(() => {
    timedOut = true;
    child.kill();
  }, FFPROBE_TIMEOUT_MS);
  try {
    const exitPromise = new Promise<number | null>((resolveExit, rejectExit) => {
      child.once("error", rejectExit);
      child.once("close", resolveExit);
    });
    const [exitResult, stdoutResult, stderrResult] = await Promise.allSettled([
      exitPromise,
      collectChildOutput(child.stdout, MAX_FFPROBE_STDOUT_BYTES, () => child.kill()),
      collectChildOutput(child.stderr, MAX_FFPROBE_STDERR_BYTES, () => child.kill()),
    ]);
    if (
      timedOut ||
      exitResult.status !== "fulfilled" ||
      exitResult.value !== 0 ||
      stdoutResult.status !== "fulfilled" ||
      stderrResult.status !== "fulfilled"
    ) {
      throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
    }
    return assertMediaMetadata(stdoutResult.value.toString("utf8"), {
      durationMs: job.expectedDurationMs,
      frameCount: job.totalFrames,
    });
  } catch (error) {
    child.kill();
    if (error instanceof WorkerFailure) {
      throw error;
    }
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  } finally {
    clearTimeout(timeout);
  }
}

async function collectChildOutput(
  stream: NodeJS.ReadableStream | null,
  maximumBytes: number,
  onLimit: () => void,
): Promise<Buffer> {
  if (stream === null) {
    throw new WorkerFailure("MEDIA_ASSERTION_FAILED");
  }

  return new Promise((resolveOutput, rejectOutput) => {
    const chunks: Buffer[] = [];
    let bytes = 0;
    let settled = false;
    stream.on("data", (chunk: Buffer | string) => {
      if (settled) {
        return;
      }
      const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      bytes += buffer.byteLength;
      if (bytes > maximumBytes) {
        settled = true;
        onLimit();
        rejectOutput(new WorkerFailure("MEDIA_ASSERTION_FAILED"));
        return;
      }
      chunks.push(buffer);
    });
    stream.once("error", (error) => {
      if (!settled) {
        settled = true;
        rejectOutput(error);
      }
    });
    stream.once("end", () => {
      if (!settled) {
        settled = true;
        resolveOutput(Buffer.concat(chunks, bytes));
      }
    });
  });
}

async function canonicalDirectory(
  path: string,
  failureCode: "INVALID_RUNTIME" | "OUTPUT_NOT_AVAILABLE",
  containingRoot?: string,
): Promise<string> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
      throw new WorkerFailure(failureCode);
    }
    const canonical = await realpath(path);
    if (
      !isWindowsLocalPath(canonical) ||
      (containingRoot !== undefined && !isInside(containingRoot, canonical))
    ) {
      throw new WorkerFailure(failureCode);
    }
    return canonical;
  } catch (error) {
    if (error instanceof WorkerFailure) {
      throw error;
    }
    throw new WorkerFailure(failureCode);
  }
}

async function regularFileWithin(root: string, path: string): Promise<string> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new WorkerFailure("INVALID_RUNTIME");
    }
    const canonical = await realpath(path);
    if (!isInside(root, canonical)) {
      throw new WorkerFailure("INVALID_RUNTIME");
    }
    return canonical;
  } catch (error) {
    if (error instanceof WorkerFailure) {
      throw error;
    }
    throw new WorkerFailure("INVALID_RUNTIME");
  }
}

async function readUtf8FileBounded(path: string, maximumBytes: number): Promise<string> {
  const handle = await open(path, "r");
  try {
    const metadata = await handle.stat();
    if (!metadata.isFile() || metadata.size < 1 || metadata.size > maximumBytes) {
      throw new WorkerFailure("INVALID_RUNTIME");
    }
    const bytes = Buffer.alloc(metadata.size);
    let offset = 0;
    while (offset < bytes.byteLength) {
      const result = await handle.read(bytes, offset, bytes.byteLength - offset, offset);
      if (result.bytesRead === 0) {
        throw new WorkerFailure("INVALID_RUNTIME");
      }
      offset += result.bytesRead;
    }
    return new TextDecoder("utf-8", {fatal: true}).decode(bytes);
  } catch (error) {
    if (error instanceof WorkerFailure) {
      throw error;
    }
    throw new WorkerFailure("INVALID_RUNTIME");
  } finally {
    await handle.close();
  }
}

function isInside(root: string, candidate: string): boolean {
  const pathFromRoot = relative(root, candidate);
  return (
    pathFromRoot.length > 0 &&
    pathFromRoot !== ".." &&
    !pathFromRoot.startsWith(`..${sep}`) &&
    !isAbsolute(pathFromRoot)
  );
}

async function pathExists(path: string): Promise<boolean> {
  try {
    await lstat(path);
    return true;
  } catch (error) {
    if (isNodeError(error, "ENOENT")) {
      return false;
    }
    throw new WorkerFailure("OUTPUT_NOT_AVAILABLE");
  }
}

function isNodeError(error: unknown, code: string): boolean {
  return (
    error instanceof Error &&
    "code" in error &&
    typeof error.code === "string" &&
    error.code === code
  );
}

function deepEqual(actual: unknown, expected: unknown): boolean {
  if (actual === expected) {
    return true;
  }
  if (Array.isArray(actual) || Array.isArray(expected)) {
    return (
      Array.isArray(actual) &&
      Array.isArray(expected) &&
      actual.length === expected.length &&
      actual.every((item, index) => deepEqual(item, expected[index]))
    );
  }
  if (!isRecord(actual) || !isRecord(expected)) {
    return false;
  }
  const actualKeys = Object.keys(actual).sort();
  const expectedKeys = Object.keys(expected).sort();
  return (
    actualKeys.length === expectedKeys.length &&
    actualKeys.every(
      (key, index) =>
        key === expectedKeys[index] && deepEqual(actual[key], expected[key]),
    )
  );
}
