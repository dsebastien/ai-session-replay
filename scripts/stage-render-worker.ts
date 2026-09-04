import {createHash} from "node:crypto";
import {spawnSync} from "node:child_process";
import {
  copyFile,
  lstat,
  mkdir,
  readFile,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import {dirname, join, resolve} from "node:path";
import {fileURLToPath} from "node:url";
import {assertPeX64, assertVersionOutput} from "./prepare-render-runtime";

export const WINDOWS_X64_TARGET = "x86_64-pc-windows-msvc" as const;

const MAX_WORKER_BUNDLE_BYTES = 16 * 1_024 * 1_024;
const SIDECAR_NAME = "render-worker-bun";
const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export interface WorkerStageReceipt {
  readonly schemaVersion: 1;
  readonly targetTriple: typeof WINDOWS_X64_TARGET;
  readonly worker: Readonly<{
    path: "worker/index.js";
    sha256: string;
  }>;
  readonly bunSidecar: "render-worker-bun.exe";
}

interface StageRenderWorkerOptions {
  readonly workerBundlePath: string;
  readonly bunExecutablePath: string;
  readonly runtimeDirectory: string;
  readonly sidecarDirectory: string;
  readonly targetTriple: string;
  readonly verifyBun: (path: string) => Promise<void>;
}

export async function stageRenderWorkerFiles(
  options: StageRenderWorkerOptions,
): Promise<WorkerStageReceipt> {
  if (options.targetTriple !== WINDOWS_X64_TARGET) {
    throw new Error(`Render worker staging requires the ${WINDOWS_X64_TARGET} x64 target`);
  }

  const worker = await readWorkerBundle(options.workerBundlePath);
  await assertRegularFile(options.bunExecutablePath, "Bun executable");
  assertPeX64(options.bunExecutablePath);
  await options.verifyBun(options.bunExecutablePath);

  const receipt: WorkerStageReceipt = {
    schemaVersion: 1,
    targetTriple: WINDOWS_X64_TARGET,
    worker: {
      path: "worker/index.js",
      sha256: createHash("sha256").update(worker).digest("hex"),
    },
    bunSidecar: "render-worker-bun.exe",
  };

  const workerDirectory = join(options.runtimeDirectory, "worker");
  const workerOutput = join(workerDirectory, "index.js");
  const receiptOutput = join(options.runtimeDirectory, "worker-layout.json");
  const sidecarOutput = join(
    options.sidecarDirectory,
    `${SIDECAR_NAME}-${WINDOWS_X64_TARGET}.exe`,
  );
  await mkdir(workerDirectory, {recursive: true});
  await mkdir(options.sidecarDirectory, {recursive: true});

  const suffix = `.partial-${process.pid}`;
  const workerTemporary = `${workerOutput}${suffix}`;
  const receiptTemporary = `${receiptOutput}${suffix}`;
  const sidecarTemporary = `${sidecarOutput}${suffix}`;
  await Promise.all([
    rm(workerTemporary, {force: true}),
    rm(receiptTemporary, {force: true}),
    rm(sidecarTemporary, {force: true}),
  ]);

  try {
    await Promise.all([
      writeFile(workerTemporary, worker, {flag: "wx"}),
      writeFile(receiptTemporary, `${JSON.stringify(receipt, null, 2)}\n`, {
        encoding: "utf8",
        flag: "wx",
      }),
      copyFile(options.bunExecutablePath, sidecarTemporary),
    ]);
    await replaceFile(workerTemporary, workerOutput);
    await replaceFile(receiptTemporary, receiptOutput);
    await replaceFile(sidecarTemporary, sidecarOutput);
  } finally {
    await Promise.all([
      rm(workerTemporary, {force: true}),
      rm(receiptTemporary, {force: true}),
      rm(sidecarTemporary, {force: true}),
    ]);
  }

  return receipt;
}

export async function stageRenderWorker(): Promise<WorkerStageReceipt> {
  if (process.platform !== "win32" || process.arch !== "x64") {
    throw new Error(
      `Render worker staging requires Windows x64, received ${process.platform}-${process.arch}`,
    );
  }

  const receipt = await stageRenderWorkerFiles({
    workerBundlePath: join(repositoryRoot, "packages", "render-worker", "dist", "index.js"),
    bunExecutablePath: join(
      repositoryRoot,
      ".runtime",
      "render-runtime",
      "bun",
      "bun.exe",
    ),
    runtimeDirectory: join(repositoryRoot, ".runtime", "render-runtime"),
    sidecarDirectory: join(repositoryRoot, ".runtime", "sidecars"),
    targetTriple: WINDOWS_X64_TARGET,
    verifyBun: verifyPinnedBun,
  });
  console.log("Staged render worker and Bun sidecar");
  return receipt;
}

async function readWorkerBundle(path: string): Promise<Buffer> {
  await assertRegularFile(path, "Render worker bundle");
  const bytes = await readFile(path);
  if (bytes.byteLength < 1 || bytes.byteLength > MAX_WORKER_BUNDLE_BYTES) {
    throw new Error("Render worker bundle has an invalid byte length");
  }
  let source: string;
  try {
    source = new TextDecoder("utf-8", {fatal: true}).decode(bytes);
  } catch {
    throw new Error("Render worker bundle must be valid UTF-8");
  }
  if (
    /^[ \t]*\/\/[#@][ \t]*sourceMappingURL=/imu.test(source) ||
    /\/\*[#@][ \t]*sourceMappingURL=.*?\*\//isu.test(source)
  ) {
    throw new Error("Render worker bundle must not reference a source map");
  }
  return bytes;
}

async function assertRegularFile(path: string, label: string): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`${label} must be a regular non-symbolic file`);
  }
}

async function replaceFile(source: string, destination: string): Promise<void> {
  await rm(destination, {force: true});
  await rename(source, destination);
}

async function verifyPinnedBun(path: string): Promise<void> {
  const result = spawnSync(path, ["--version"], {
    encoding: "utf8",
    env: {...process.env, PATH: ""},
    timeout: 30_000,
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error("Staged Bun failed its version check");
  }
  assertVersionOutput("Bun", `${result.stdout}\n${result.stderr}`, "1.4.0");
}

if (import.meta.main) {
  await stageRenderWorker();
}
