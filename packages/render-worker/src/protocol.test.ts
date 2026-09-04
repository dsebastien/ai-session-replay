import type {PresentationPlanV1} from "@ai-session-replay/replay-contract";
import contractFixture from "../../../tests/fixtures/indexed-library-contracts-v1.json";
import {mkdir, mkdtemp, readFile, realpath, rm, stat, writeFile} from "node:fs/promises";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {afterEach, describe, expect, it} from "vitest";
import {
  ProtocolWriter,
  WorkerFailure,
  classifyWorkerError,
  parseRenderRequestLine,
  readRenderRequest,
  safeJobId,
  type ProtocolEvent,
} from "./protocol";
import {runWorker} from "./index";
import {
  assertMediaMetadata,
  forceRemotionLoopbackBinding,
  isBrowserUrlAllowed,
  prepareRenderJob,
  removeStagingOutput,
  restrictBrowserPage,
} from "./render";

const plan = contractFixture.presentationPlan as PresentationPlanV1;

const validRequest = {
  schemaVersion: 1,
  type: "render",
  jobId: "job-123",
  outputPath: "D:\\exports\\replay.mp4",
  stagingPath: "D:\\exports\\.ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4",
  plan,
} as const;

function chunks(...values: string[]): AsyncIterable<Uint8Array> {
  return {
    async *[Symbol.asyncIterator]() {
      for (const value of values) {
        yield new TextEncoder().encode(value);
      }
    },
  };
}

describe("render request protocol", () => {
  it("validates and normalizes a complete request", () => {
    const parsed = parseRenderRequestLine(JSON.stringify(validRequest));

    expect(parsed).toEqual(validRequest);
    expect(parsed.plan).not.toBe(plan);
  });

  it.each([
    [{...validRequest, jobId: "bad\njob"}, "INVALID_REQUEST"],
    [{...validRequest, outputPath: "relative.mp4"}, "INVALID_REQUEST"],
    [{...validRequest, outputPath: "D:\\exports\\bad?.mp4"}, "INVALID_REQUEST"],
    [{...validRequest, outputPath: "D:\\exports\\replay.webm"}, "INVALID_REQUEST"],
    [{...validRequest, stagingPath: "D:\\other\\.ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4"}, "INVALID_REQUEST"],
    [{...validRequest, stagingPath: "D:\\exports\\predictable.partial.mp4"}, "INVALID_REQUEST"],
    [{...validRequest, plan: {...plan, width: 1280}}, "INVALID_REQUEST"],
    [{...validRequest, unexpected: true}, "INVALID_REQUEST"],
  ])("rejects malformed request data before rendering", (candidate, code) => {
    expect(() => parseRenderRequestLine(JSON.stringify(candidate))).toThrowError(
      expect.objectContaining({code}),
    );
  });

  it("rejects raw control characters", () => {
    expect(() => parseRenderRequestLine(`{"schemaVersion":1}\u0001`)).toThrowError(
      expect.objectContaining({code: "INVALID_REQUEST"}),
    );
  });

  it("bounds the request line before parsing JSON", async () => {
    await expect(
      readRenderRequest(chunks("123456789"), {
        maxLineBytes: 8,
        maxStreamBytes: 16,
      }),
    ).rejects.toMatchObject({code: "REQUEST_LIMIT_EXCEEDED"});
  });

  it("bounds the full stdin stream", async () => {
    await expect(
      readRenderRequest(chunks("{}\n", "padding"), {
        maxLineBytes: 4,
        maxStreamBytes: 8,
      }),
    ).rejects.toMatchObject({code: "REQUEST_LIMIT_EXCEEDED"});
  });

  it("accepts exactly one newline-terminated request", async () => {
    const parsed = await readRenderRequest(
      chunks(JSON.stringify(validRequest), "\r\n"),
    );

    expect(parsed.jobId).toBe("job-123");
    await expect(
      readRenderRequest(chunks(`${JSON.stringify(validRequest)}\n{}\n`)),
    ).rejects.toMatchObject({code: "INVALID_REQUEST"});
  });
});

describe("worker output protocol", () => {
  it("emits bounded job-scoped NDJSON and one terminal event", () => {
    const lines: string[] = [];
    const writer = new ProtocolWriter((line) => lines.push(line));

    writer.started("job-123", 45);
    writer.progress("job-123", 10, 45);
    writer.succeeded("job-123", {
      codec: "h264",
      width: 1920,
      height: 1080,
      pixelFormat: "yuv420p",
      durationMs: 1_500,
      frameCount: 45,
    }, "a".repeat(64));
    writer.failed("job-123", "WORKER_INTERNAL");

    const events = lines.map((line) => JSON.parse(line) as ProtocolEvent);
    expect(events.map((event) => event.type)).toEqual([
      "started",
      "progress",
      "succeeded",
    ]);
    expect(events.every((event) => event.jobId === "job-123")).toBe(true);
    expect(lines.every((line) => line.endsWith("\n"))).toBe(true);
  });

  it("enforces event, line, and total stdout limits", () => {
    const eventWriter = new ProtocolWriter(() => undefined, {
      maxEvents: 2,
      maxLineBytes: 1_024,
      maxBytes: 4_096,
    });
    eventWriter.started("job-123", 45);
    expect(() => eventWriter.progress("job-123", 1, 45)).toThrowError(
      expect.objectContaining({code: "PROTOCOL_LIMIT_EXCEEDED"}),
    );

    const lineWriter = new ProtocolWriter(() => undefined, {
      maxEvents: 4,
      maxLineBytes: 16,
      maxBytes: 4_096,
    });
    expect(() => lineWriter.started("job-123", 45)).toThrowError(
      expect.objectContaining({code: "PROTOCOL_LIMIT_EXCEEDED"}),
    );

    const byteWriter = new ProtocolWriter(() => undefined, {
      maxEvents: 4,
      maxLineBytes: 1_024,
      maxBytes: 100,
    });
    byteWriter.started("job-123", 45);
    expect(() => byteWriter.succeeded("job-123", {
      codec: "h264",
      width: 1920,
      height: 1080,
      pixelFormat: "yuv420p",
      durationMs: 1_500,
      frameCount: 45,
    }, "a".repeat(64))).toThrowError(expect.objectContaining({code: "PROTOCOL_LIMIT_EXCEEDED"}));
    const invalidDigestWriter = new ProtocolWriter(() => undefined);
    expect(() => invalidDigestWriter.succeeded("another-job", {
      codec: "h264",
      width: 1920,
      height: 1080,
      pixelFormat: "yuv420p",
      durationMs: 1_500,
      frameCount: 45,
    }, "not-a-digest")).toThrowError(expect.objectContaining({code: "PROTOCOL_LIMIT_EXCEEDED"}));
  });

  it("never includes raw exception or transcript text in failures", () => {
    const secret = "private transcript and C:\\Users\\person\\session.jsonl";
    const lines: string[] = [];
    const writer = new ProtocolWriter((line) => lines.push(line));

    writer.failed("job-123", classifyWorkerError(new Error(secret)));

    expect(lines.join("")).not.toContain(secret);
    expect(lines).toEqual([
      '{"schemaVersion":1,"type":"failed","jobId":"job-123","code":"WORKER_INTERNAL"}\n',
    ]);
    expect(safeJobId(secret)).toBe("unassigned");
  });
});

describe("render security and media assertions", () => {
  it.each([
    ["http://127.0.0.1:3000/index.html", true],
    ["http://localhost:3000/bundle.js", true],
    ["http://[::1]:3000/index.html", true],
    ["data:text/plain,ok", true],
    ["blob:http://127.0.0.1:3000/id", true],
    ["http://127.0.0.1:3001/private-service", false],
    ["https://example.com/asset", false],
    ["http://192.168.1.2/asset", false],
    ["file:///C:/secret.txt", false],
    ["not a URL", false],
  ])("allows only loopback browser URLs: %s", (url, allowed) => {
    expect(isBrowserUrlAllowed(url, 3000)).toBe(allowed);
  });

  it("accepts exact H.264 1080p yuv420p metadata", () => {
    expect(
      assertMediaMetadata(
        JSON.stringify({
          streams: [{
            codec_name: "h264",
            width: 1920,
            height: 1080,
            pix_fmt: "yuv420p",
            nb_read_frames: "45",
            duration: "1.500000",
          }],
        }),
        {durationMs: 1_500, frameCount: 45},
      ),
    ).toEqual({
      codec: "h264",
      width: 1920,
      height: 1080,
      pixelFormat: "yuv420p",
      durationMs: 1_500,
      frameCount: 45,
    });
  });

  it.each([
    [{codec_name: "hevc", width: 1920, height: 1080, pix_fmt: "yuv420p", nb_read_frames: "45", duration: "1.500000"}],
    [{codec_name: "h264", width: 1280, height: 1080, pix_fmt: "yuv420p", nb_read_frames: "45", duration: "1.500000"}],
    [{codec_name: "h264", width: 1920, height: 1080, pix_fmt: "yuvj420p", nb_read_frames: "45", duration: "1.500000"}],
    [{codec_name: "h264", width: 1920, height: 1080, pix_fmt: "yuv420p", nb_read_frames: "44", duration: "1.500000"}],
    [{codec_name: "h264", width: 1920, height: 1080, pix_fmt: "yuv420p", nb_read_frames: "45", duration: "2.000000"}],
  ])("rejects mismatched media metadata", (stream) => {
    expect(() => assertMediaMetadata(
      JSON.stringify({streams: [stream]}),
      {durationMs: 1_500, frameCount: 45},
    )).toThrowError(expect.objectContaining({code: "MEDIA_ASSERTION_FAILED"}));
  });

  it("keeps worker failure codes on a fixed allowlist", () => {
    expect(classifyWorkerError(new WorkerFailure("INVALID_RUNTIME"))).toBe(
      "INVALID_RUNTIME",
    );
    expect(classifyWorkerError({code: "PRIVATE_TRANSCRIPT"})).toBe(
      "WORKER_INTERNAL",
    );
  });
});

describe("render preparation", () => {
  const temporaryDirectories: string[] = [];

  afterEach(async () => {
    await Promise.all(
      temporaryDirectories.splice(0).map((directory) =>
        rm(directory, {recursive: true, force: true})
      ),
    );
  });

  it("resolves a verified runtime and output before browser launch", async () => {
    const root = await createSyntheticRuntime();
    const outputDirectory = join(root, "output");
    await mkdir(outputDirectory);
    const request = {
      ...validRequest,
      outputPath: join(outputDirectory, "replay.mp4"),
      stagingPath: join(outputDirectory, ".ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4"),
    };

    const prepared = await prepareRenderJob(request, join(root, "runtime"));

    expect(prepared.totalFrames).toBe(150);
    expect(prepared.expectedDurationMs).toBe(5_000);
    expect(prepared.paths.browserExecutable).toBe(await realpath(
      join(root, "runtime", "chrome-headless-shell", "chrome-headless-shell.exe"),
    ));
    expect(prepared.paths.ffprobeExecutable).toBe(await realpath(
      join(root, "runtime", "remotion", "ffprobe.exe"),
    ));
  });

  it("rejects altered runtime layout paths", async () => {
    const root = await createSyntheticRuntime({
      browserExecutable: "../outside.exe",
    });
    const outputDirectory = join(root, "output");
    await mkdir(outputDirectory);

    await expect(
      prepareRenderJob(
        {
          ...validRequest,
          outputPath: join(outputDirectory, "replay.mp4"),
          stagingPath: join(outputDirectory, ".ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4"),
        },
        join(root, "runtime"),
      ),
    ).rejects.toMatchObject({code: "INVALID_RUNTIME"});
  });

  it("classifies malformed runtime layout JSON as an invalid runtime", async () => {
    const root = await createSyntheticRuntime();
    const outputDirectory = join(root, "output");
    await mkdir(outputDirectory);
    await writeFile(join(root, "runtime", "runtime-layout.json"), "{", "utf8");

    await expect(
      prepareRenderJob(
        {
          ...validRequest,
          outputPath: join(outputDirectory, "replay.mp4"),
          stagingPath: join(outputDirectory, ".ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4"),
        },
        join(root, "runtime"),
      ),
    ).rejects.toMatchObject({code: "INVALID_RUNTIME"});
  });

  it("rejects an existing output before browser launch", async () => {
    const root = await createSyntheticRuntime();
    const outputDirectory = join(root, "output");
    await mkdir(outputDirectory);
    const outputPath = join(outputDirectory, "replay.mp4");
    await writeFile(outputPath, "existing", "utf8");

    await expect(
      prepareRenderJob({...validRequest, outputPath}, join(root, "runtime")),
    ).rejects.toMatchObject({code: "OUTPUT_NOT_AVAILABLE"});
  });

  it("removes failed staging data", async () => {
    const root = await mkdtemp(join(tmpdir(), "asr-worker-output-test-"));
    temporaryDirectories.push(root);
    const failedStaging = join(root, ".ai-session-replay-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.partial.mp4");
    await writeFile(failedStaging, "partial private content", "utf8");
    await removeStagingOutput(failedStaging);
    await expect(stat(failedStaging)).rejects.toBeDefined();
  });

  async function createSyntheticRuntime(
    pathOverride: Readonly<{browserExecutable?: string}> = {},
  ): Promise<string> {
    const root = await mkdtemp(join(tmpdir(), "asr-worker-test-"));
    temporaryDirectories.push(root);
    const runtime = join(root, "runtime");
    await mkdir(join(runtime, "bun"), {recursive: true});
    await mkdir(join(runtime, "chrome-headless-shell"), {recursive: true});
    await mkdir(join(runtime, "composition"), {recursive: true});
    await mkdir(join(runtime, "remotion"), {recursive: true});
    await Promise.all([
      writeFile(join(runtime, "bun", "bun.exe"), "fixture", "utf8"),
      writeFile(
        join(runtime, "chrome-headless-shell", "chrome-headless-shell.exe"),
        "fixture",
        "utf8",
      ),
      writeFile(join(runtime, "composition", "index.html"), "fixture", "utf8"),
      writeFile(join(runtime, "remotion", "remotion.exe"), "fixture", "utf8"),
      writeFile(join(runtime, "remotion", "ffmpeg.exe"), "fixture", "utf8"),
      writeFile(join(runtime, "remotion", "ffprobe.exe"), "fixture", "utf8"),
    ]);
    await writeFile(
      join(runtime, "runtime-layout.json"),
      JSON.stringify({
        schemaVersion: 1,
        platform: "windows-x64",
        versions: {
          bun: "1.4.0",
          chromeHeadlessShell: "149.0.7790.0",
          remotion: "4.0.516",
        },
        paths: {
          bunExecutable: "bun/bun.exe",
          browserExecutable:
            pathOverride.browserExecutable ??
            "chrome-headless-shell/chrome-headless-shell.exe",
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
      }),
      "utf8",
    );
    return root;
  }
});

describe("Remotion network containment", () => {
  it("forces Remotion's server configuration to IPv4 loopback", () => {
    expect(forceRemotionLoopbackBinding()).toEqual({
      host: "127.0.0.1",
      hostsToTry: ["127.0.0.1"],
    });
  });

  it("continues loopback requests and blocks external requests before resolution", async () => {
    const commands: Array<Readonly<{method: string; params?: unknown}>> = [];
    const handlers = new Map<string, (event: unknown) => void>();
    const session = {
      on(event: string, handler: (event: unknown) => void) {
        handlers.set(event, handler);
      },
      async send(method: string, params?: unknown) {
        commands.push({method, params});
      },
    };

    await restrictBrowserPage({_client: () => session}, 3000);
    handlers.get("Fetch.requestPaused")?.({
      requestId: "local",
      request: {url: "http://127.0.0.1:3000/bundle.js"},
    });
    handlers.get("Fetch.requestPaused")?.({
      requestId: "other-local",
      request: {url: "http://127.0.0.1:3001/private-service"},
    });
    handlers.get("Fetch.requestPaused")?.({
      requestId: "external",
      request: {url: "https://example.com/tracker"},
    });
    await new Promise((resolve) => queueMicrotask(resolve));

    expect(commands).toContainEqual({
      method: "Fetch.enable",
      params: {patterns: [{requestStage: "Request", urlPattern: "*"}]},
    });
    expect(commands).toContainEqual({
      method: "Fetch.continueRequest",
      params: {requestId: "local"},
    });
    expect(commands).toContainEqual({
      method: "Fetch.failRequest",
      params: {errorReason: "BlockedByClient", requestId: "other-local"},
    });
    expect(commands).toContainEqual({
      method: "Fetch.failRequest",
      params: {errorReason: "BlockedByClient", requestId: "external"},
    });
  });
});

describe("worker lifecycle", () => {
  it("does not prepare or launch rendering for an invalid request", async () => {
    const lines: string[] = [];
    let prepareCalls = 0;

    const exitCode = await runWorker(
      chunks("{}\n"),
      "C:\\runtime",
      (line) => lines.push(line),
      {
        async prepare() {
          prepareCalls += 1;
          throw new Error("must not run");
        },
        async render() {
          throw new Error("must not run");
        },
      },
    );

    expect(exitCode).toBe(1);
    expect(prepareCalls).toBe(0);
    expect(lines).toEqual([
      '{"schemaVersion":1,"type":"failed","jobId":"unassigned","code":"INVALID_REQUEST"}\n',
    ]);
  });

  it("reduces render exceptions to a terminal allowlisted failure", async () => {
    const lines: string[] = [];
    const secret = "raw transcript should never leave the worker";

    const exitCode = await runWorker(
      chunks(`${JSON.stringify(validRequest)}\n`),
      "C:\\runtime",
      (line) => lines.push(line),
      {
        async prepare(request) {
          return {
            request,
            totalFrames: 45,
            expectedDurationMs: 1_500,
            paths: {
              browserExecutable: "C:\\runtime\\chrome.exe",
              binariesDirectory: "C:\\runtime\\remotion",
              compositionDirectory: "C:\\runtime\\composition",
              ffprobeExecutable: "C:\\runtime\\ffprobe.exe",
              outputPath: request.outputPath,
              stagingPath: request.stagingPath,
            },
          };
        },
        async render() {
          throw new Error(secret);
        },
      },
    );

    expect(exitCode).toBe(1);
    expect(lines.join("")).not.toContain(secret);
    expect(lines.map((line) => (JSON.parse(line) as ProtocolEvent).type)).toEqual([
      "started",
      "failed",
    ]);
  });
});
