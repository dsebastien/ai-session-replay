// @vitest-environment jsdom

import {render} from "@testing-library/react";
import {describe, expect, it} from "vitest";
import type {ThemeSpec} from "@ai-session-replay/replay-contract";
import {TerminalText} from "./TerminalText";

const theme: ThemeSpec = {
  background: "#101211",
  surface: "#171A18",
  text: "#F5F5F4",
  muted: "#9B9E9C",
  accent: "#D6AA68",
  success: "#A8D5BD",
  error: "#E59A91",
};

describe("TerminalText", () => {
  it("renders SGR console colors without exposing control codes", () => {
    const view = render(
      <TerminalText
        text={"\u001b[37mme\u001b[90m,\u001b[0m \u001b[92m$_\u001b[37m.summary\u001b[0m \u001b[37m}\u001b[0m"}
        theme={theme}
      />,
    );

    expect(view.container.textContent).toBe("me, $_.summary }");
    expect(view.container.textContent).not.toContain("[37m");
    expect([...view.container.querySelectorAll("span")].map((span) => span.style.color))
      .toEqual([
        "rgb(245, 245, 244)",
        "rgb(155, 158, 156)",
        "rgb(168, 213, 189)",
        "rgb(245, 245, 244)",
        "rgb(245, 245, 244)",
      ]);
  });

  it("strips terminal hyperlinks and cursor controls while keeping inert text", () => {
    const view = render(
      <TerminalText
        text={"before \u001b]8;;https://example.invalid\u0007click\u001b]8;;\u0007\u001b[2K after"}
        theme={theme}
      />,
    );

    expect(view.container.textContent).toBe("before click after");
    expect(view.container.querySelector("a")).toBeNull();
  });

  it("supports indexed and true-color SGR values with bounded RGB channels", () => {
    const view = render(
      <TerminalText
        text={"\u001b[38;5;196mred\u001b[0m \u001b[38;2;12;34;56mcustom\u001b[0m"}
        theme={theme}
      />,
    );
    const spans = [...view.container.querySelectorAll("span")];

    expect(spans[0]?.style.color).toBe("rgb(255, 0, 0)");
    expect(spans[1]?.style.color).toBe("rgb(12, 34, 56)");
  });
});
