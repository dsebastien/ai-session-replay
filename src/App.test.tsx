import {renderToStaticMarkup} from "react-dom/server";
import {describe, expect, it} from "vitest";
import {App} from "./App";

describe("App", () => {
  it("identifies the product and renders the startup loading screen immediately", () => {
    const markup = renderToStaticMarkup(<App />);

    expect(markup).toContain("<h1>AI Session Replay</h1>");
    expect(markup).toContain("On-device");
    expect(markup).toContain("Preparing your local library");
    expect(markup).toContain("Discovering and indexing local AI sessions.");
    expect(markup).toContain('aria-label="Loading AI Session Replay"');
  });
});
