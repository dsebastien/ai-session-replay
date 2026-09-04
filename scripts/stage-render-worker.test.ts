import {createHash} from "node:crypto";
import {mkdir, mkdtemp, readFile, rm, writeFile} from "node:fs/promises";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {afterEach, describe, expect, it} from "vitest";
import {
  WINDOWS_X64_TARGET,
  stageRenderWorkerFiles,
} from "./stage-render-worker";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((directory) =>
      rm(directory, {recursive: true, force: true})
    ),
  );
});

describe("stageRenderWorkerFiles", () => {
  it("stages the worker resource and target-triple Bun sidecar", async () => {
    const root = await temporaryDirectory();
    const workerBundlePath = join(root, "source", "index.js");
    const bunExecutablePath = join(root, "source", "bun.exe");
    const runtimeDirectory = join(root, "runtime");
    const sidecarDirectory = join(root, "sidecars");
    await mkdir(join(root, "source"), {recursive: true});
    await writeFile(workerBundlePath, "console.log('synthetic worker');\n", "utf8");
    await writeFile(bunExecutablePath, fakePeX64());

    const receipt = await stageRenderWorkerFiles({
      workerBundlePath,
      bunExecutablePath,
      runtimeDirectory,
      sidecarDirectory,
      targetTriple: WINDOWS_X64_TARGET,
      verifyBun: async () => undefined,
    });

    const stagedWorker = join(runtimeDirectory, "worker", "index.js");
    const stagedBun = join(
      sidecarDirectory,
      `render-worker-bun-${WINDOWS_X64_TARGET}.exe`,
    );
    expect(await readFile(stagedWorker, "utf8")).toBe(
      "console.log('synthetic worker');\n",
    );
    expect(await readFile(stagedBun)).toEqual(fakePeX64());
    expect(receipt).toEqual({
      schemaVersion: 1,
      targetTriple: WINDOWS_X64_TARGET,
      worker: {
        path: "worker/index.js",
        sha256: createHash("sha256")
          .update("console.log('synthetic worker');\n")
          .digest("hex"),
      },
      bunSidecar: "render-worker-bun.exe",
    });
    expect(
      JSON.parse(
        await readFile(join(runtimeDirectory, "worker-layout.json"), "utf8"),
      ),
    ).toEqual(receipt);
  });

  it("rejects a source map that could disclose build-machine paths", async () => {
    const root = await temporaryDirectory();
    const source = join(root, "source");
    await mkdir(source);
    await writeFile(
      join(source, "index.js"),
      "console.log('worker');\n//# sourceMappingURL=index.js.map\n",
      "utf8",
    );
    await writeFile(join(source, "bun.exe"), fakePeX64());

    await expect(
      stageRenderWorkerFiles({
        workerBundlePath: join(source, "index.js"),
        bunExecutablePath: join(source, "bun.exe"),
        runtimeDirectory: join(root, "runtime"),
        sidecarDirectory: join(root, "sidecars"),
        targetTriple: WINDOWS_X64_TARGET,
        verifyBun: async () => undefined,
      }),
    ).rejects.toThrow(/source map/u);
  });

  it("rejects unsupported target triples before writing output", async () => {
    const root = await temporaryDirectory();

    await expect(
      stageRenderWorkerFiles({
        workerBundlePath: join(root, "missing-worker.js"),
        bunExecutablePath: join(root, "missing-bun.exe"),
        runtimeDirectory: join(root, "runtime"),
        sidecarDirectory: join(root, "sidecars"),
        targetTriple: "aarch64-pc-windows-msvc",
        verifyBun: async () => undefined,
      }),
    ).rejects.toThrow(/x64 target/u);
  });
});

async function temporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "asr-worker-stage-"));
  temporaryDirectories.push(directory);
  return directory;
}

function fakePeX64(): Buffer {
  const bytes = Buffer.alloc(128);
  bytes.write("MZ", 0, "ascii");
  bytes.writeUInt32LE(64, 0x3c);
  bytes.write("PE\0\0", 64, "binary");
  bytes.writeUInt16LE(0x8664, 68);
  return bytes;
}
