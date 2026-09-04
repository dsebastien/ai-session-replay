// @vitest-environment jsdom

import {render} from "@testing-library/react";
import {describe, expect, it} from "vitest";
import type {PresentationEntryContentV1, ThemeSpec} from "@ai-session-replay/replay-contract";
import {ConversationCard} from "./ConversationCard";

const theme: ThemeSpec = {
  background: "#101211", surface: "#171A18", text: "#F5F5F4", muted: "#9B9E9C",
  accent: "#D6AA68", success: "#A8D5BD", error: "#E59A91",
};

describe("ConversationCard", () => {
  it("describes a genuinely unknown record without claiming it is unsupported", () => {
    const view = render(<ConversationCard
      entry={{entryKey: "future", ordinal: 0, atMs: 0, kind: "unknown", sourceType: "future-record"}}
      mode="conversation"
      theme={theme}
    />);

    expect(view.getByText("Unrecognized source event")).toBeDefined();
    expect(view.container.textContent).not.toContain("Unsupported");
  });

  it.each<PresentationEntryContentV1>([
    {entryKey: "user", ordinal: 0, atMs: 0, kind: "user", text: "<script>inert</script>"},
    {entryKey: "reason", ordinal: 1, atMs: 1, kind: "reasoning", text: "Explicit trace"},
    {entryKey: "tool", ordinal: 2, atMs: 2, kind: "tool-call", name: "read", status: "succeeded", summary: "Read", detail: {arguments: "{}", result: "ok"}},
  ])("keeps review and video content structurally identical for $kind", (entry) => {
    const review = render(<ConversationCard entry={entry} mode="conversation" theme={theme} />);
    const reviewCard = review.getByTestId(`entry-${entry.entryKey}`);
    const text = reviewCard.textContent;
    const style = reviewCard.getAttribute("style");
    review.unmount();
    const video = render(<ConversationCard entry={entry} mode="video" theme={theme} />);
    const videoCard = video.getByTestId(`entry-${entry.entryKey}`);

    expect(videoCard.textContent).toBe(text);
    expect(style).toContain(entry.kind === "reasoning" || entry.kind === "tool-call"
      ? "width: 100%"
      : "width: min(82%, 760px)");
    expect(videoCard.style.width).toBe("100%");
    expect(video.container.querySelector("script, img, a")).toBeNull();
  });

  it("formats ANSI content consistently in summaries and expanded tool details", () => {
    const view = render(<ConversationCard
      entry={{
        entryKey: "ansi-tool",
        ordinal: 0,
        atMs: 0,
        kind: "tool-call",
        name: "shell_command",
        status: "succeeded",
        summary: "\u001b[92mCommand succeeded\u001b[0m",
        detail: {
          arguments: "\u001b[37mrg --files\u001b[0m",
          result: "\u001b[90msrc/main.ts\u001b[0m",
        },
      }}
      mode="conversation"
      theme={theme}
    />);

    expect(view.container.textContent).toContain("Command succeeded");
    expect(view.container.textContent).toContain("rg --files");
    expect(view.container.textContent).toContain("src/main.ts");
    expect(view.container.textContent).not.toContain("[92m");
    expect(view.container.querySelectorAll("[data-terminal-style]").length).toBe(3);
  });
});
