import {createHash} from "node:crypto";
import {spawnSync} from "node:child_process";
import {closeSync, createReadStream, existsSync, openSync, readSync} from "node:fs";
import {lstat, mkdir, open, readFile, readdir, rename, rm, stat, writeFile} from "node:fs/promises";
import {basename, dirname, extname, join, resolve} from "node:path";
import {fileURLToPath} from "node:url";

const EXPECTED_PLATFORM = "windows-x64" as const;
const SHA256_PATTERN = /^[a-f0-9]{64}$/u;
const VERSION_PATTERN = /\b\d+\.\d+\.\d+(?:\.\d+)?\b/u;

const ARTIFACT_CONTRACTS = {
  bun: {
    version: "1.4.0",
    url: "https://github.com/oven-sh/bun/releases/download/bun-v1.4.0/bun-windows-x64.zip",
    size: 40_060_247,
    format: "zip",
    archiveRoot: "bun-windows-x64",
  },
  chromeHeadlessShell: {
    version: "149.0.7790.0",
    url: "https://storage.googleapis.com/chrome-for-testing-public/149.0.7790.0/win64/chrome-headless-shell-win64.zip",
    size: 118_785_025,
    format: "zip",
    archiveRoot: "chrome-headless-shell-win64",
  },
  remotionCompositor: {
    version: "4.0.516",
    url: "https://registry.npmjs.org/@remotion/compositor-win32-x64-msvc/-/compositor-win32-x64-msvc-4.0.516.tgz",
    size: 11_101_157,
    format: "tgz",
    archiveRoot: "package",
  },
} as const;

type ArtifactId = keyof typeof ARTIFACT_CONTRACTS;
type ArchiveFormat = "zip" | "tgz";

export interface RuntimeArtifact {
  readonly version: string;
  readonly url: string;
  readonly sha256: string;
  readonly size: number;
  readonly format: ArchiveFormat;
  readonly archiveRoot: string;
}

export interface RuntimeManifest {
  readonly schemaVersion: 1;
  readonly platform: typeof EXPECTED_PLATFORM;
  readonly artifacts: Readonly<Record<ArtifactId, RuntimeArtifact>>;
}

interface PinnedFile {
  readonly sha256: string;
  readonly size: number;
}

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const cacheDirectory = join(repositoryRoot, ".tools", "render-runtime-cache");
const runtimeDirectory = join(repositoryRoot, ".runtime", "render-runtime");
const manifestPath = join(repositoryRoot, "scripts", "runtime-manifest.json");
const artifactDestinations: Readonly<Record<ArtifactId, string>> = {
  bun: "bun",
  chromeHeadlessShell: "chrome-headless-shell",
  remotionCompositor: "remotion",
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertExactKeys(value: Record<string, unknown>, expected: readonly string[], label: string): void {
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    throw new Error(`${label} must contain exactly: ${wanted.join(", ")}`);
  }
}

function validateArtifact(value: unknown, id: ArtifactId): RuntimeArtifact {
  if (!isRecord(value)) {
    throw new Error(`Runtime artifact ${id} must be an object`);
  }
  assertExactKeys(value, ["version", "url", "sha256", "size", "format", "archiveRoot"], `Runtime artifact ${id}`);

  const expected = ARTIFACT_CONTRACTS[id];
  for (const field of ["version", "url", "size", "format", "archiveRoot"] as const) {
    if (value[field] !== expected[field]) {
      throw new Error(`Runtime artifact ${id}.${field} must be ${String(expected[field])}`);
    }
  }

  if (typeof value.sha256 !== "string" || !SHA256_PATTERN.test(value.sha256)) {
    throw new Error(`Runtime artifact ${id}.sha256 must be a lowercase SHA-256 digest`);
  }

  const parsedUrl = new URL(value.url as string);
  if (parsedUrl.protocol !== "https:") {
    throw new Error(`Runtime artifact ${id}.url must use HTTPS`);
  }

  return {
    version: value.version as string,
    url: value.url as string,
    sha256: value.sha256,
    size: value.size as number,
    format: value.format as ArchiveFormat,
    archiveRoot: value.archiveRoot as string,
  };
}

export function validateRuntimeManifest(value: unknown): RuntimeManifest {
  if (!isRecord(value)) {
    throw new Error("Runtime manifest must be an object");
  }
  assertExactKeys(value, ["schemaVersion", "platform", "artifacts"], "Runtime manifest");
  if (value.schemaVersion !== 1) {
    throw new Error("Runtime manifest schemaVersion must be 1");
  }
  if (value.platform !== EXPECTED_PLATFORM) {
    throw new Error(`Runtime manifest platform must be ${EXPECTED_PLATFORM}`);
  }
  if (!isRecord(value.artifacts)) {
    throw new Error("Runtime manifest artifacts must be an object");
  }
  assertExactKeys(value.artifacts, Object.keys(ARTIFACT_CONTRACTS), "Runtime manifest artifacts");

  return {
    schemaVersion: 1,
    platform: EXPECTED_PLATFORM,
    artifacts: {
      bun: validateArtifact(value.artifacts.bun, "bun"),
      chromeHeadlessShell: validateArtifact(value.artifacts.chromeHeadlessShell, "chromeHeadlessShell"),
      remotionCompositor: validateArtifact(value.artifacts.remotionCompositor, "remotionCompositor"),
    },
  };
}

export async function verifyPinnedFile(path: string, expected: PinnedFile): Promise<void> {
  const metadata = await stat(path);
  if (metadata.size !== expected.size) {
    throw new Error(`Artifact size mismatch for ${path}: expected ${expected.size}, received ${metadata.size}`);
  }

  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) {
    hash.update(chunk as Buffer);
  }
  const actual = hash.digest("hex");
  if (actual !== expected.sha256) {
    throw new Error(`Artifact SHA-256 mismatch for ${path}: expected ${expected.sha256}, received ${actual}`);
  }
}

export function assertPeX64(path: string): void {
  const descriptor = openSync(path, "r");
  try {
    const dosHeader = Buffer.alloc(64);
    if (readSync(descriptor, dosHeader, 0, dosHeader.length, 0) !== dosHeader.length || dosHeader.toString("ascii", 0, 2) !== "MZ") {
      throw new Error(`${path} is not a valid PE image`);
    }

    const peOffset = dosHeader.readUInt32LE(0x3c);
    const peHeader = Buffer.alloc(6);
    if (readSync(descriptor, peHeader, 0, peHeader.length, peOffset) !== peHeader.length || peHeader.toString("binary", 0, 4) !== "PE\0\0") {
      throw new Error(`${path} is not a valid PE image`);
    }

    const machine = peHeader.readUInt16LE(4);
    if (machine !== 0x8664) {
      throw new Error(`${path} is not a Windows x64 PE image (machine 0x${machine.toString(16)})`);
    }
  } finally {
    closeSync(descriptor);
  }
}

export function assertVersionOutput(label: string, output: string, expectedVersion: string): void {
  const actualVersion = output.match(VERSION_PATTERN)?.[0];
  if (actualVersion !== expectedVersion) {
    throw new Error(`${label} version mismatch: expected ${expectedVersion}, received ${actualVersion ?? "no version"}`);
  }
}

function artifactFileName(artifact: RuntimeArtifact): string {
  const name = basename(new URL(artifact.url).pathname);
  if (!name || name === "." || name === "..") {
    throw new Error(`Runtime artifact URL has no safe filename: ${artifact.url}`);
  }
  return name;
}

async function downloadPinnedArtifact(id: ArtifactId, artifact: RuntimeArtifact): Promise<string> {
  await mkdir(cacheDirectory, {recursive: true});
  const destination = join(cacheDirectory, artifactFileName(artifact));

  if (existsSync(destination)) {
    try {
      await verifyPinnedFile(destination, artifact);
      console.log(`Verified cached ${id} archive`);
      return destination;
    } catch {
      await rm(destination, {force: true});
    }
  }

  const temporaryPath = `${destination}.partial-${process.pid}`;
  await rm(temporaryPath, {force: true});
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5 * 60 * 1_000);

  try {
    console.log(`Downloading pinned ${id} ${artifact.version}`);
    const response = await fetch(artifact.url, {redirect: "follow", signal: controller.signal});
    if (!response.ok || !response.body) {
      throw new Error(`Download failed for ${id}: HTTP ${response.status}`);
    }
    if (new URL(response.url).protocol !== "https:") {
      throw new Error(`Download for ${id} redirected away from HTTPS`);
    }
    const declaredLength = response.headers.get("content-length");
    if (declaredLength !== null && Number(declaredLength) !== artifact.size) {
      throw new Error(`Download size mismatch for ${id}: expected ${artifact.size}, received ${declaredLength}`);
    }

    const destinationFile = await open(temporaryPath, "wx");
    let received = 0;
    try {
      const reader = response.body.getReader();
      while (true) {
        const chunk = await reader.read();
        if (chunk.done) {
          break;
        }
        received += chunk.value.byteLength;
        if (received > artifact.size) {
          await reader.cancel("Artifact exceeded its pinned byte length");
          throw new Error(`Download for ${id} exceeded its pinned byte length`);
        }
        await destinationFile.write(chunk.value);
      }
    } finally {
      await destinationFile.close();
    }

    if (received !== artifact.size) {
      throw new Error(`Download size mismatch for ${id}: expected ${artifact.size}, received ${received}`);
    }
    await verifyPinnedFile(temporaryPath, artifact);
    await rename(temporaryPath, destination);
    return destination;
  } catch (error) {
    await rm(temporaryPath, {force: true});
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

function powershellExecutable(): string {
  const windowsDirectory = process.env.SystemRoot ?? process.env.WINDIR;
  if (!windowsDirectory) {
    throw new Error("SystemRoot is not available; cannot extract a ZIP archive safely");
  }
  return join(windowsDirectory, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
}

async function extractArtifact(archivePath: string, artifact: RuntimeArtifact, destination: string): Promise<void> {
  await mkdir(destination, {recursive: true});
  if (artifact.format === "tgz") {
    const archive = new Bun.Archive(await readFile(archivePath));
    await archive.extract(destination);
    return;
  }

  const result = spawnSync(
    powershellExecutable(),
    [
      "-NoLogo",
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "$ErrorActionPreference='Stop'; Expand-Archive -LiteralPath $env:ASR_ARCHIVE -DestinationPath $env:ASR_DESTINATION -Force",
    ],
    {
      encoding: "utf8",
      env: {...process.env, ASR_ARCHIVE: archivePath, ASR_DESTINATION: destination},
      timeout: 120_000,
      windowsHide: true,
    },
  );
  if (result.status !== 0) {
    throw new Error(`ZIP extraction failed: ${(result.stderr || result.stdout).trim()}`);
  }
}

async function collectFiles(directory: string): Promise<string[]> {
  const files: string[] = [];
  for (const entry of await readdir(directory, {withFileTypes: true})) {
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink()) {
      throw new Error(`Symbolic links are not allowed in staged runtime artifacts: ${path}`);
    }
    if (entry.isDirectory()) {
      files.push(...(await collectFiles(path)));
    } else if (entry.isFile()) {
      files.push(path);
    }
  }
  return files;
}

async function renameDirectoryWithRetries(source: string, destination: string): Promise<void> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      await rename(source, destination);
      return;
    } catch (error) {
      const code = error instanceof Error && "code" in error ? String(error.code) : "";
      if (attempt >= 19 || (code !== "EPERM" && code !== "EACCES")) {
        throw error;
      }
      await new Promise<void>((resolveDelay) => setTimeout(resolveDelay, 250));
    }
  }
}

function scrubbedPathEnvironment(): NodeJS.ProcessEnv {
  return {...process.env, PATH: ""};
}

function executableVersion(path: string, args: readonly string[] = ["--version"]): string {
  const result = spawnSync(path, args, {
    encoding: "utf8",
    env: scrubbedPathEnvironment(),
    timeout: 30_000,
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error(`${path} failed its version check: ${(result.stderr || result.stdout).trim()}`);
  }
  return `${result.stdout}\n${result.stderr}`;
}

async function validateStagedArtifact(id: ArtifactId, artifact: RuntimeArtifact, directory: string): Promise<void> {
  const files = await collectFiles(directory);
  const peFiles = files.filter((path) => [".exe", ".dll"].includes(extname(path).toLowerCase()));
  if (peFiles.length === 0) {
    throw new Error(`Staged ${id} contains no PE files`);
  }
  for (const path of peFiles) {
    assertPeX64(path);
  }

  if (id === "bun") {
    assertVersionOutput("Bun", executableVersion(join(directory, "bun.exe")), artifact.version);
    return;
  }
  if (id === "chromeHeadlessShell") {
    assertVersionOutput(
      "Chrome Headless Shell",
      executableVersion(join(directory, "chrome-headless-shell.exe")),
      artifact.version,
    );
    return;
  }

  const packageJson = JSON.parse(await readFile(join(directory, "package.json"), "utf8")) as {
    name?: unknown;
    version?: unknown;
  };
  if (packageJson.name !== "@remotion/compositor-win32-x64-msvc" || packageJson.version !== artifact.version) {
    throw new Error(`Remotion compositor package version mismatch: expected ${artifact.version}`);
  }
  const ffmpegVersion = executableVersion(join(directory, "ffmpeg.exe"), ["-version"]);
  const ffprobeVersion = executableVersion(join(directory, "ffprobe.exe"), ["-version"]);
  if (!ffmpegVersion.includes("ffmpeg version n7.1 ") || !ffprobeVersion.includes("ffprobe version n7.1 ")) {
    throw new Error("Remotion compositor contains an unexpected FFmpeg build");
  }
}

async function stageArtifact(id: ArtifactId, artifact: RuntimeArtifact, archivePath: string): Promise<void> {
  const extractionDirectory = join(repositoryRoot, ".runtime", `.extract-${id}-${process.pid}`);
  const stagingDirectory = join(repositoryRoot, ".runtime", `.stage-${id}-${process.pid}`);
  const destination = join(runtimeDirectory, artifactDestinations[id]);
  await rm(extractionDirectory, {recursive: true, force: true});
  await rm(stagingDirectory, {recursive: true, force: true});

  try {
    await extractArtifact(archivePath, artifact, extractionDirectory);
    const extractedRoot = join(extractionDirectory, artifact.archiveRoot);
    const extractedMetadata = await lstat(extractedRoot);
    if (!extractedMetadata.isDirectory()) {
      throw new Error(`Archive root is not a directory for ${id}: ${artifact.archiveRoot}`);
    }
    await renameDirectoryWithRetries(extractedRoot, stagingDirectory);
    await validateStagedArtifact(id, artifact, stagingDirectory);
    await rm(destination, {recursive: true, force: true});
    await renameDirectoryWithRetries(stagingDirectory, destination);
    console.log(`Staged and verified ${id}`);
  } finally {
    await rm(extractionDirectory, {recursive: true, force: true});
    await rm(stagingDirectory, {recursive: true, force: true});
  }
}

async function writeRuntimeReceipt(manifest: RuntimeManifest): Promise<void> {
  const compositionDirectory = join(runtimeDirectory, "composition");
  const compositionIndex = join(compositionDirectory, "index.html");
  if (!existsSync(compositionIndex)) {
    throw new Error("Composition bundle is missing; run the composition build before runtime preparation");
  }
  const html = await readFile(compositionIndex, "utf8");
  if (/(?:src|href)=["']\//u.test(html)) {
    throw new Error("Composition bundle contains an origin-root asset URL and is not relocatable");
  }

  const receipt = {
    schemaVersion: 1,
    platform: manifest.platform,
    versions: {
      bun: manifest.artifacts.bun.version,
      chromeHeadlessShell: manifest.artifacts.chromeHeadlessShell.version,
      remotion: manifest.artifacts.remotionCompositor.version,
    },
    paths: {
      bunExecutable: "bun/bun.exe",
      browserExecutable: "chrome-headless-shell/chrome-headless-shell.exe",
      binariesDirectory: "remotion",
      composition: "composition",
    },
    archiveSha256: {
      bun: manifest.artifacts.bun.sha256,
      chromeHeadlessShell: manifest.artifacts.chromeHeadlessShell.sha256,
      remotionCompositor: manifest.artifacts.remotionCompositor.sha256,
    },
  } as const;
  const output = join(runtimeDirectory, "runtime-layout.json");
  const temporaryOutput = `${output}.partial-${process.pid}`;
  await writeFile(temporaryOutput, `${JSON.stringify(receipt, null, 2)}\n`, "utf8");
  await rm(output, {force: true});
  await rename(temporaryOutput, output);
}

export async function prepareRenderRuntime(): Promise<string> {
  if (process.platform !== "win32" || process.arch !== "x64") {
    throw new Error(`Render runtime preparation requires Windows x64, received ${process.platform}-${process.arch}`);
  }

  const manifest = validateRuntimeManifest(JSON.parse(await readFile(manifestPath, "utf8")) as unknown);
  await mkdir(runtimeDirectory, {recursive: true});
  for (const id of Object.keys(ARTIFACT_CONTRACTS) as ArtifactId[]) {
    const artifact = manifest.artifacts[id];
    const archivePath = await downloadPinnedArtifact(id, artifact);
    await stageArtifact(id, artifact, archivePath);
  }
  await writeRuntimeReceipt(manifest);
  console.log(`Render runtime staged at ${runtimeDirectory}`);
  return runtimeDirectory;
}

if (import.meta.main) {
  await prepareRenderRuntime();
}
