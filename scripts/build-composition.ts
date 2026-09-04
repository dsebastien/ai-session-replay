import {existsSync} from "node:fs";
import {cp, mkdir, readFile, rename, rm, writeFile} from "node:fs/promises";
import {dirname, join, resolve} from "node:path";
import {fileURLToPath} from "node:url";
import {bundle} from "@remotion/bundler";
import {
  REMOTION_BUNDLE_DIRECTORY,
  REMOTION_ENTRY_POINT,
  REMOTION_PUBLIC_DIRECTORY,
} from "../remotion.config";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputDirectory = join(repositoryRoot, REMOTION_BUNDLE_DIRECTORY);

export function makeCompositionHtmlRelocatable(html: string, buildRoot: string): string {
  const cwdAssignment = `window.remotion_cwd = ${JSON.stringify(buildRoot)};`;
  if (!html.includes(cwdAssignment)) {
    throw new Error("Remotion bundle does not contain the expected build-root metadata");
  }
  const relocated = html.replace(cwdAssignment, 'window.remotion_cwd = ".";');
  if (/(?:src|href)=["']\//u.test(relocated)) {
    throw new Error("Remotion bundle contains an origin-root asset URL and is not relocatable");
  }
  return relocated;
}

export async function buildComposition(): Promise<string> {
  const stagingDirectory = join(repositoryRoot, ".runtime", `.composition-${process.pid}`);
  await rm(stagingDirectory, {recursive: true, force: true});
  await mkdir(dirname(outputDirectory), {recursive: true});

  try {
    const result = await bundle({
      entryPoint: join(repositoryRoot, REMOTION_ENTRY_POINT),
      outDir: stagingDirectory,
      rootDir: repositoryRoot,
      publicDir: join(repositoryRoot, REMOTION_PUBLIC_DIRECTORY),
      publicPath: "./",
      enableCaching: false,
      webpackOverride: (configuration) => ({...configuration, devtool: false}),
      onProgress: (progress) => {
        if (progress === 100 || progress % 25 === 0) {
          console.log(`Composition bundle: ${progress}%`);
        }
      },
    });

    if (resolve(result) !== resolve(stagingDirectory) || !existsSync(join(stagingDirectory, "index.html"))) {
      throw new Error("Remotion did not produce the expected relocatable bundle");
    }
    if (!existsSync(join(stagingDirectory, "public", "fonts", "JetBrainsMono.woff2"))) {
      throw new Error("Remotion bundle is missing the packaged font");
    }

    const indexPath = join(stagingDirectory, "index.html");
    const relocatedHtml = makeCompositionHtmlRelocatable(await readFile(indexPath, "utf8"), repositoryRoot);
    await writeFile(indexPath, relocatedHtml, "utf8");

    await rm(outputDirectory, {recursive: true, force: true});
    await renameWithRetry(stagingDirectory, outputDirectory);
    console.log(`Composition bundle staged at ${outputDirectory}`);
    return outputDirectory;
  } catch (error) {
    await rm(stagingDirectory, {recursive: true, force: true});
    throw error;
  }
}

async function renameWithRetry(source: string, destination: string): Promise<void> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      await rename(source, destination);
      return;
    } catch (error) {
      if (!isWindowsRenameSharingViolation(error)) throw error;
      if (attempt >= 24) {
        await cp(source, destination, {recursive: true, errorOnExist: true, force: false});
        await rm(source, {recursive: true, force: true});
        return;
      }
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
    }
  }
}

function isWindowsRenameSharingViolation(error: unknown): boolean {
  return process.platform === "win32" &&
    typeof error === "object" && error !== null &&
    "code" in error && (error.code === "EPERM" || error.code === "EBUSY");
}

if (import.meta.main) {
  await buildComposition();
}
