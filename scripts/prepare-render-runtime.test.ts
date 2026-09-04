import {createHash} from "node:crypto";
import {mkdtempSync, readFileSync, rmSync, writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {afterEach, describe, expect, it} from "vitest";
import {
  assertPeX64,
  assertVersionOutput,
  validateRuntimeManifest,
  verifyPinnedFile,
} from "./prepare-render-runtime";

const temporaryDirectories: string[] = [];

function temporaryDirectory(): string {
  const directory = mkdtempSync(join(tmpdir(), "ai-session-replay-runtime-"));
  temporaryDirectories.push(directory);
  return directory;
}

function validManifest(): unknown {
  return {
    schemaVersion: 1,
    platform: "windows-x64",
    artifacts: {
      bun: {
        version: "1.4.0",
        url: "https://github.com/oven-sh/bun/releases/download/bun-v1.4.0/bun-windows-x64.zip",
        sha256: "a".repeat(64),
        size: 40_060_247,
        format: "zip",
        archiveRoot: "bun-windows-x64",
      },
      chromeHeadlessShell: {
        version: "149.0.7790.0",
        url: "https://storage.googleapis.com/chrome-for-testing-public/149.0.7790.0/win64/chrome-headless-shell-win64.zip",
        sha256: "b".repeat(64),
        size: 118_785_025,
        format: "zip",
        archiveRoot: "chrome-headless-shell-win64",
      },
      remotionCompositor: {
        version: "4.0.516",
        url: "https://registry.npmjs.org/@remotion/compositor-win32-x64-msvc/-/compositor-win32-x64-msvc-4.0.516.tgz",
        sha256: "c".repeat(64),
        size: 11_101_157,
        format: "tgz",
        archiveRoot: "package",
      },
    },
  };
}

function fakePe(machine: number): Buffer {
  const bytes = Buffer.alloc(128);
  bytes.write("MZ", 0, "ascii");
  bytes.writeUInt32LE(64, 0x3c);
  bytes.write("PE\0\0", 64, "binary");
  bytes.writeUInt16LE(machine, 68);
  return bytes;
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, {recursive: true, force: true});
  }
});

describe("validateRuntimeManifest", () => {
  it("accepts the committed pinned manifest", () => {
    const manifest = JSON.parse(readFileSync(new URL("./runtime-manifest.json", import.meta.url), "utf8")) as unknown;

    expect(() => validateRuntimeManifest(manifest)).not.toThrow();
  });

  it("accepts the fixed Windows x64 runtime contract", () => {
    const manifest = validateRuntimeManifest(validManifest());

    expect(manifest.artifacts.bun.version).toBe("1.4.0");
    expect(manifest.artifacts.chromeHeadlessShell.version).toBe("149.0.7790.0");
    expect(manifest.artifacts.remotionCompositor.version).toBe("4.0.516");
  });

  it.each([
    ["unpinned Bun", ["artifacts", "bun", "version"], "latest"],
    ["non-HTTPS URL", ["artifacts", "bun", "url"], "http://example.com/bun.zip"],
    ["unapproved host", ["artifacts", "bun", "url"], "https://example.com/bun.zip"],
    ["malformed hash", ["artifacts", "bun", "sha256"], "not-a-sha256"],
    ["unsafe archive root", ["artifacts", "bun", "archiveRoot"], "../outside"],
  ])("rejects %s", (_label, path, replacement) => {
    const manifest = validManifest() as Record<string, unknown>;
    let cursor = manifest;
    for (const segment of path.slice(0, -1)) {
      cursor = cursor[segment] as Record<string, unknown>;
    }
    cursor[path.at(-1)!] = replacement;

    expect(() => validateRuntimeManifest(manifest)).toThrow();
  });
});

describe("verifyPinnedFile", () => {
  it("accepts only the declared byte length and SHA-256", async () => {
    const path = join(temporaryDirectory(), "artifact.bin");
    const contents = Buffer.from("verified artifact", "utf8");
    writeFileSync(path, contents);
    const sha256 = createHash("sha256").update(contents).digest("hex");

    await expect(verifyPinnedFile(path, {sha256, size: contents.byteLength})).resolves.toBeUndefined();
    await expect(verifyPinnedFile(path, {sha256: "0".repeat(64), size: contents.byteLength})).rejects.toThrow(
      /SHA-256/,
    );
    await expect(verifyPinnedFile(path, {sha256, size: contents.byteLength + 1})).rejects.toThrow(/size/);
  });
});

describe("assertPeX64", () => {
  it("accepts an AMD64 PE image", () => {
    const path = join(temporaryDirectory(), "x64.exe");
    writeFileSync(path, fakePe(0x8664));

    expect(() => assertPeX64(path)).not.toThrow();
  });

  it("rejects a non-x64 or malformed image", () => {
    const directory = temporaryDirectory();
    const armPath = join(directory, "arm64.exe");
    const malformedPath = join(directory, "malformed.exe");
    writeFileSync(armPath, fakePe(0xaa64));
    writeFileSync(malformedPath, Buffer.from("not PE"));

    expect(() => assertPeX64(armPath)).toThrow(/x64/);
    expect(() => assertPeX64(malformedPath)).toThrow(/PE/);
  });
});

describe("assertVersionOutput", () => {
  it("requires exact Bun and Chrome versions", () => {
    expect(() => assertVersionOutput("Bun", "1.4.0\n", "1.4.0")).not.toThrow();
    expect(() =>
      assertVersionOutput("Chrome Headless Shell", "Google Chrome for Testing 149.0.7790.0\n", "149.0.7790.0"),
    ).not.toThrow();
    expect(() => assertVersionOutput("Bun", "1.4.1\n", "1.4.0")).toThrow(/1\.4\.0/);
  });
});
