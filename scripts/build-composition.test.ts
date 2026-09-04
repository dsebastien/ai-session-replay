import {describe, expect, it} from "vitest";
import {makeCompositionHtmlRelocatable} from "./build-composition";

describe("makeCompositionHtmlRelocatable", () => {
  it("removes the build-machine root while preserving relative assets", () => {
    const root = String.raw`C:\source\ai-session-replay`;
    const html = `<script>window.remotion_cwd = ${JSON.stringify(root)};</script><script src="./bundle.js"></script>`;

    const result = makeCompositionHtmlRelocatable(html, root);

    expect(result).not.toContain(root);
    expect(result).toContain('window.remotion_cwd = ".";');
    expect(result).toContain('src="./bundle.js"');
  });

  it("rejects origin-root asset references", () => {
    const root = String.raw`C:\source\ai-session-replay`;
    const html = `<script>window.remotion_cwd = ${JSON.stringify(root)};</script><script src="/bundle.js"></script>`;

    expect(() => makeCompositionHtmlRelocatable(html, root)).toThrow(/relocatable/);
  });
});
