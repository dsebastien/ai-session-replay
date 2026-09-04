import {existsSync} from "node:fs";
import {dirname, join, resolve} from "node:path";
import {fileURLToPath} from "node:url";
import {inflateSync} from "node:zlib";
import {createServer} from "node:net";
import {openBrowser, renderStill, selectComposition} from "@remotion/renderer";
import type {PresentationPlanV1} from "../packages/replay-contract/src";
import fixture from "../tests/fixtures/indexed-library-contracts-v1.json";
import {
  forceRemotionLoopbackBinding,
  installLoopbackNetworkPolicy,
  reserveLoopbackPort,
} from "../packages/render-worker/src/render";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const browserExecutable = join(root, ".runtime", "render-runtime", "chrome-headless-shell", "chrome-headless-shell.exe");
const binariesDirectory = join(root, ".runtime", "render-runtime", "remotion");
const serveUrl = join(root, ".runtime", "render-runtime", "composition");
const frames = [0, 29, 30, 60, 90, 149] as const;
const plan = richPlan();

if (!existsSync(browserExecutable) || !existsSync(binariesDirectory) || !existsSync(join(serveUrl, "index.html"))) {
  throw new Error("The pinned render runtime is missing; run bun run prepare:render-runtime first");
}

forceRemotionLoopbackBinding();
const loopbackPort = await reserveLoopbackPort();
const browser = await openBrowser("chrome", {
  browserExecutable,
  chromeMode: "headless-shell",
  chromiumOptions: {headless: true},
  logLevel: "error",
});
installLoopbackNetworkPolicy(browser, loopbackPort);

try {
  await assertDisallowedLoopbackPortIsBlocked(browser, loopbackPort);
  const inputProps = {plan};
  const common = {
    serveUrl,
    inputProps,
    puppeteerInstance: browser,
    browserExecutable,
    binariesDirectory,
    chromeMode: "headless-shell" as const,
    logLevel: "error" as const,
    port: loopbackPort,
  };
  const presentation = await selectComposition({...common, id: "PresentationEquivalence"});
  const exported = await selectComposition({...common, id: "TerminalReplay"});
  assertMatchingMetadata(presentation, exported);
  console.log("Presentation/export metadata matches.");

  for (const frame of frames) {
    console.log(`Comparing frame ${frame}…`);
    const presentationPng = await renderStill({
      ...common,
      composition: presentation,
      frame,
      imageFormat: "png",
      output: null,
    });
    const exportPng = await renderStill({
      ...common,
      composition: exported,
      frame,
      imageFormat: "png",
      output: null,
    });
    if (!presentationPng.buffer || !exportPng.buffer) {
      throw new Error(`Frame ${frame} did not produce an in-memory PNG`);
    }
    assertPixelsEquivalent(
      decodePng(presentationPng.buffer),
      decodePng(exportPng.buffer),
      frame,
    );
  }
} finally {
  // Remotion's Browser.close() asks every accumulated still-render page for a
  // Page object after terminating Chrome; that can wait indefinitely on this
  // pinned Windows build. Close the owned process first, then disconnect the
  // already-dead protocol transport.
  await browser.runner.closeProcess();
  browser.disconnect();
}

console.log(`Presentation/export pixel equivalence passed at ${frames.length} representative frames.`);

async function assertDisallowedLoopbackPortIsBlocked(
  browser: Awaited<ReturnType<typeof openBrowser>>,
  allowedPort: number,
): Promise<void> {
  let receivedConnection = false;
  const server = createServer((socket) => {
    receivedConnection = true;
    socket.destroy();
  });
  await new Promise<void>((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  try {
    const address = server.address();
    if (typeof address !== "object" || address === null || address.port === allowedPort) {
      throw new Error("Could not reserve a disallowed loopback test port");
    }
    const page = await browser.newPage({
      context: () => null,
      logLevel: "error",
      indent: false,
      pageIndex: 999,
      onBrowserLog: null,
      onLog: () => undefined,
    });
    try {
      await page.goto({
        url: `http://127.0.0.1:${address.port}/must-not-connect`,
        timeout: 2_000,
      }).catch(() => null);
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
      if (receivedConnection) throw new Error("Chrome reached a disallowed loopback port");
      console.log("Chrome blocked a disallowed loopback endpoint.");
    } finally {
      await page.close();
    }
  } finally {
    await new Promise<void>((resolveClose, rejectClose) => {
      server.close((error) => error ? rejectClose(error) : resolveClose());
    });
  }
}

function richPlan(): PresentationPlanV1 {
  const base = fixture.presentationPlan as PresentationPlanV1;
  return {
    ...base,
    entries: base.entries.map((item) => item.entry.kind === "tool-call"
      ? {...item, entry: {...item.entry, detail: {arguments: "{\"path\":\"synthetic.txt\"}", result: "ok"}}}
      : item),
    preferences: {
      ...base.preferences,
      visibility: {...base.preferences.visibility, showToolDetails: true},
    },
  };
}

function assertMatchingMetadata(
  presentation: Readonly<{width: number; height: number; fps: number; durationInFrames: number}>,
  exported: Readonly<{width: number; height: number; fps: number; durationInFrames: number}>,
): void {
  for (const key of ["width", "height", "fps", "durationInFrames"] as const) {
    if (presentation[key] !== exported[key]) {
      throw new Error(`Presentation/export ${key} differs`);
    }
  }
}

function decodePng(png: Buffer): Buffer {
  if (!png.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) {
    throw new Error("Rendered frame is not a PNG");
  }
  let offset = 8;
  let width = 0;
  let height = 0;
  let bytesPerPixel = 0;
  const imageChunks: Buffer[] = [];
  while (offset + 12 <= png.length) {
    const length = png.readUInt32BE(offset);
    const type = png.toString("ascii", offset + 4, offset + 8);
    const dataStart = offset + 8;
    const dataEnd = dataStart + length;
    if (dataEnd + 4 > png.length) throw new Error("Rendered PNG is truncated");
    if (type === "IHDR") {
      width = png.readUInt32BE(dataStart);
      height = png.readUInt32BE(dataStart + 4);
      const bitDepth = png[dataStart + 8];
      const colorType = png[dataStart + 9];
      const interlace = png[dataStart + 12];
      if (bitDepth !== 8 || (colorType !== 2 && colorType !== 6) || interlace !== 0) {
        throw new Error("Rendered PNG uses an unsupported pixel layout");
      }
      bytesPerPixel = colorType === 6 ? 4 : 3;
    } else if (type === "IDAT") {
      imageChunks.push(png.subarray(dataStart, dataEnd));
    } else if (type === "IEND") {
      break;
    }
    offset = dataEnd + 4;
  }
  if (width !== plan.width || height !== plan.height || bytesPerPixel === 0) {
    throw new Error("Rendered PNG dimensions do not match the frozen plan");
  }
  const packed = inflateSync(Buffer.concat(imageChunks));
  const stride = width * bytesPerPixel;
  if (packed.length !== (stride + 1) * height) {
    throw new Error("Rendered PNG scanline length is invalid");
  }
  const decoded = Buffer.alloc(stride * height);
  for (let row = 0; row < height; row += 1) {
    const packedOffset = row * (stride + 1);
    const outputOffset = row * stride;
    const filter = packed[packedOffset];
    for (let column = 0; column < stride; column += 1) {
      const source = packed[packedOffset + column + 1] ?? 0;
      const left = column >= bytesPerPixel ? (decoded[outputOffset + column - bytesPerPixel] ?? 0) : 0;
      const above = row > 0 ? (decoded[outputOffset + column - stride] ?? 0) : 0;
      const upperLeft = row > 0 && column >= bytesPerPixel
        ? (decoded[outputOffset + column - stride - bytesPerPixel] ?? 0)
        : 0;
      decoded[outputOffset + column] = unfilter(source, filter, left, above, upperLeft);
    }
  }
  return decoded;
}

function unfilter(source: number, filter: number | undefined, left: number, above: number, upperLeft: number): number {
  switch (filter) {
    case 0: return source;
    case 1: return source + left;
    case 2: return source + above;
    case 3: return source + Math.floor((left + above) / 2);
    case 4: return source + paeth(left, above, upperLeft);
    default: throw new Error("Rendered PNG uses an unknown scanline filter");
  }
}

function paeth(left: number, above: number, upperLeft: number): number {
  const prediction = left + above - upperLeft;
  const leftDistance = Math.abs(prediction - left);
  const aboveDistance = Math.abs(prediction - above);
  const upperLeftDistance = Math.abs(prediction - upperLeft);
  if (leftDistance <= aboveDistance && leftDistance <= upperLeftDistance) return left;
  return aboveDistance <= upperLeftDistance ? above : upperLeft;
}

function assertPixelsEquivalent(presentation: Buffer, exported: Buffer, frame: number): void {
  if (presentation.length !== exported.length) {
    throw new Error(`Frame ${frame} decoded to different pixel counts`);
  }
  let changedChannels = 0;
  let largestDelta = 0;
  for (let index = 0; index < presentation.length; index += 1) {
    const delta = Math.abs((presentation[index] ?? 0) - (exported[index] ?? 0));
    if (delta > 0) changedChannels += 1;
    if (delta > largestDelta) largestDelta = delta;
  }
  const changedRatio = changedChannels / presentation.length;
  if (largestDelta > 1 || changedRatio > 0.0001) {
    throw new Error(
      `Frame ${frame} differs: max channel delta ${largestDelta}, changed ratio ${changedRatio}`,
    );
  }
}
