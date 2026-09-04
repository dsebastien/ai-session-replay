import {writeSync} from "node:fs";
import {
  ProtocolWriter,
  classifyWorkerError,
  readRenderRequest,
  safeJobId,
  type RenderRequest,
} from "./protocol";
import {
  prepareRenderJob,
  renderPreparedJob,
  type PreparedRenderJob,
  type RenderedMedia,
} from "./render";

interface WorkerDependencies {
  readonly prepare: (
    request: RenderRequest,
    runtimeDirectory: string,
  ) => Promise<PreparedRenderJob>;
  readonly render: (
    job: PreparedRenderJob,
    onProgress: (renderedFrames: number, totalFrames: number) => void,
  ) => Promise<RenderedMedia>;
}

const DEFAULT_DEPENDENCIES: WorkerDependencies = {
  prepare: prepareRenderJob,
  render: renderPreparedJob,
};

export async function runWorker(
  source: AsyncIterable<Uint8Array | string>,
  runtimeDirectory: string,
  write: (line: string) => void,
  dependencies: WorkerDependencies = DEFAULT_DEPENDENCIES,
): Promise<0 | 1> {
  const protocol = new ProtocolWriter(write);
  let jobId = "unassigned";

  try {
    const request = await readRenderRequest(source);
    jobId = request.jobId;
    const prepared = await dependencies.prepare(request, runtimeDirectory);
    protocol.started(jobId, prepared.totalFrames);
    const rendered = await dependencies.render(
      prepared,
      (renderedFrames, totalFrames) =>
        protocol.progress(jobId, renderedFrames, totalFrames),
    );
    protocol.succeeded(jobId, rendered.media, rendered.stagingSha256);
    return 0;
  } catch (error) {
    try {
      protocol.failed(safeJobId(jobId), classifyWorkerError(error));
    } catch {
      // A closed pipe or exhausted protocol budget cannot be reported safely.
    }
    return 1;
  }
}

function runtimeDirectoryArgument(args: readonly string[]): string {
  return args.length === 2 && args[0] === "--runtime-directory"
    ? (args[1] ?? "")
    : "";
}

function silenceUntrustedOutput(): void {
  const discard = () => true;
  process.stdout.write = discard as typeof process.stdout.write;
  process.stderr.write = discard as typeof process.stderr.write;
  console.debug = () => undefined;
  console.error = () => undefined;
  console.info = () => undefined;
  console.log = () => undefined;
  console.warn = () => undefined;
}

if (import.meta.main) {
  silenceUntrustedOutput();
  const exitCode = await runWorker(
    process.stdin,
    runtimeDirectoryArgument(process.argv.slice(2)),
    (line) => {
      writeSync(1, line);
    },
  );
  process.exit(exitCode);
}
